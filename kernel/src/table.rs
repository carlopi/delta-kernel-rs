//! In-memory representation of a Delta table, which acts as an immutable root entity for reading
//! the different versions

use std::path::PathBuf;

use url::Url;

use crate::snapshot::Snapshot;
use crate::{DeltaResult, Engine, Error, Version};

/// In-memory representation of a Delta table, which acts as an immutable root entity for reading
/// the different versions (see [`Snapshot`]) of the table located in storage.
#[derive(Clone)]
pub struct Table {
    location: Url,
}

impl std::fmt::Debug for Table {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("Table")
            .field("location", &self.location)
            .finish()
    }
}

impl Table {
    /// Create a new Delta table with the given parameters
    pub fn new(location: Url) -> Self {
        Self { location }
    }

    /// Try to create a new table from a string uri. This will do it's best to handle things like
    /// `/local/paths`, and even `../relative/paths`.
    pub fn try_from_uri(uri: impl AsRef<str>) -> DeltaResult<Self> {
        let uri = uri.as_ref();
        let uri_type: UriType = resolve_uri_type(uri)?;
        let url = match uri_type {
            UriType::LocalPath(path) => {
                if !path.exists() {
                    // When we support writes, create a directory if we can
                    return Err(Error::InvalidTableLocation(format!(
                        "Path does not exist: {path:?}"
                    )));
                }
                if !path.is_dir() {
                    return Err(Error::InvalidTableLocation(format!(
                        "{path:?} is not a directory"
                    )));
                }
                let path = std::fs::canonicalize(path).map_err(|err| {
                    let msg = format!("Invalid table location: {} Error: {:?}", uri, err);
                    Error::InvalidTableLocation(msg)
                })?;
                Url::from_directory_path(path.clone()).map_err(|_| {
                    let msg = format!(
                        "Could not construct a URL from canonicalized path: {:?}.\n\
                         Something must be very wrong with the table path.",
                        path
                    );
                    Error::InvalidTableLocation(msg)
                })?
            }
            UriType::Url(url) => url,
        };
        Ok(Self::new(url))
    }

    /// Fully qualified location of the Delta table.
    pub fn location(&self) -> &Url {
        &self.location
    }

    /// Create a [`Snapshot`] of the table corresponding to `version`.
    ///
    /// If no version is supplied, a snapshot for the latest version will be created.
    pub fn snapshot(&self, engine: &dyn Engine, version: Option<Version>) -> DeltaResult<Snapshot> {
        Snapshot::try_new(self.location.clone(), engine, version)
    }
}

#[derive(Debug)]
enum UriType {
    LocalPath(PathBuf),
    Url(Url),
}

/// Utility function to figure out whether string representation of the path is either local path or
/// some kind or URL.
///
/// Will return an error if the path is not valid.
fn resolve_uri_type(table_uri: impl AsRef<str>) -> DeltaResult<UriType> {
    let table_uri = table_uri.as_ref();
    if let Ok(url) = Url::parse(table_uri) {
        let scheme = url.scheme().to_string();
        if url.scheme() == "file" {
            Ok(UriType::LocalPath(
                url.to_file_path()
                    .map_err(|_| Error::invalid_table_location(table_uri))?,
            ))
        } else if scheme.len() == 1 {
            // NOTE this check is required to support absolute windows paths which may properly
            // parse as url we assume here that a single character scheme is a windows drive letter
            Ok(UriType::LocalPath(PathBuf::from(table_uri)))
        } else {
            Ok(UriType::Url(url))
        }
    } else {
        Ok(UriType::LocalPath(table_uri.into()))
    }
}
