use std::collections::HashSet;
use std::sync::Arc;

use super::data_skipping::DataSkippingFilter;
use crate::actions::action_definitions::{Add, AddVisitor, Remove, RemoveVisitor};
use crate::engine_data::{ExtractInto, GetDataItem};
use crate::expressions::Expression;
use crate::schema::{SchemaRef, StructType};
use crate::{DataExtractor, DataVisitor, DeltaResult, EngineData};

use either::Either;
use tracing::debug;

struct LogReplayScanner {
    filter: Option<DataSkippingFilter>,

    /// A set of (data file path, dv_unique_id) pairs that have been seen thus
    /// far in the log. This is used to filter out files with Remove actions as
    /// well as duplicate entries in the log.
    seen: HashSet<(String, Option<String>)>,
}

#[derive(Default)]
struct AddRemoveVisitor {
    adds: Vec<Add>,
    removes: Vec<Remove>,
}

impl DataVisitor for AddRemoveVisitor {
    fn visit<'a>(
        &mut self,
        row_count: usize,
        getters: &[&'a dyn GetDataItem<'a>],
    ) -> DeltaResult<()> {
        println!("at top: {}", getters.len());
        for i in 0..row_count {
            // Add will have a path at index 0 if it is valid
            if let Some(path) = getters[0].extract_into_opt(i, "add.path")? {
                self.adds.push(AddVisitor::visit_add(i, path, getters)?);
            }
            // Remove will have a path at index 15 if it is valid
            // TODO(nick): Should count the fields in Add to ensure we don't get this wrong if more
            // are added
            if let Some(path) = getters[15].extract_into_opt(i, "remove.path")? {
                let remove_getters = &getters[15..];
                self.removes
                    .push(RemoveVisitor::visit_remove(i, path, remove_getters)?);
            }
        }
        Ok(())
    }
}

impl LogReplayScanner {
    /// Create a new [`LogReplayStream`] instance
    fn new(table_schema: &SchemaRef, predicate: &Option<Expression>) -> Self {
        Self {
            filter: DataSkippingFilter::new(table_schema, predicate),
            seen: Default::default(),
        }
    }

    /// Extract Add actions from a single batch. This will filter out rows that
    /// don't match the predicate and Add actions that have corresponding Remove
    /// actions in the log.
    fn process_batch(
        &mut self,
        actions: &dyn EngineData,
        data_extractor: &Arc<dyn DataExtractor>,
        is_log_batch: bool,
    ) -> DeltaResult<Vec<Add>> {
        let filtered_actions = self
            .filter
            .as_ref()
            .map(|filter| filter.apply(actions))
            .transpose()?;
        let actions = match filtered_actions {
            Some(ref filtered_actions) => filtered_actions.as_ref(),
            None => actions,
        };

        let schema_to_use = StructType::new(if is_log_batch {
            vec![
                crate::actions::schemas::ADD_FIELD.clone(),
                crate::actions::schemas::REMOVE_FIELD.clone(),
            ]
        } else {
            // All checkpoint actions are already reconciled and Remove actions in checkpoint files
            // only serve as tombstones for vacuum jobs. So no need to load them here.
            vec![crate::actions::schemas::ADD_FIELD.clone()]
        });
        let mut add_remove_visitor = AddRemoveVisitor::default();
        data_extractor.extract(actions, Arc::new(schema_to_use), &mut add_remove_visitor)?;

        for remove in add_remove_visitor.removes.into_iter() {
            self.seen
                .insert((remove.path.clone(), remove.dv_unique_id()));
        }

        add_remove_visitor
            .adds
            .into_iter()
            .filter_map(|add| {
                // Note: each (add.path + add.dv_unique_id()) pair has a
                // unique Add + Remove pair in the log. For example:
                // https://github.com/delta-io/delta/blob/master/spark/src/test/resources/delta/table-with-dv-large/_delta_log/00000000000000000001.json
                if !self.seen.contains(&(add.path.clone(), add.dv_unique_id())) {
                    debug!("Found file: {}, is log {}", &add.path, is_log_batch);
                    if is_log_batch {
                        // Remember file actions from this batch so we can ignore duplicates
                        // as we process batches from older commit and/or checkpoint files. We
                        // don't need to track checkpoint batches because they are already the
                        // oldest actions and can never replace anything.
                        self.seen.insert((add.path.clone(), add.dv_unique_id()));
                    }
                    Some(Ok(add))
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Given an iterator of (record batch, bool) tuples and a predicate, returns an iterator of `Adds`.
/// The boolean flag indicates whether the record batch is a log or checkpoint batch.
pub fn log_replay_iter(
    action_iter: impl Iterator<Item = DeltaResult<(Box<dyn EngineData>, bool)>>,
    data_extractor: Arc<dyn DataExtractor>,
    table_schema: &SchemaRef,
    predicate: &Option<Expression>,
) -> impl Iterator<Item = DeltaResult<Add>> {
    let mut log_scanner = LogReplayScanner::new(table_schema, predicate);

    action_iter.flat_map(move |actions| match actions {
        Ok((batch, is_log_batch)) => {
            match log_scanner.process_batch(batch.as_ref(), &data_extractor, is_log_batch) {
                Ok(adds) => Either::Left(adds.into_iter().map(Ok)),
                Err(err) => Either::Right(std::iter::once(Err(err))),
            }
        }
        Err(err) => Either::Right(std::iter::once(Err(err))),
    })
}
