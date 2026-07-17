// Licensed to the Apache Software Foundation (ASF) under one
// or more contributor license agreements.  See the NOTICE file
// distributed with this work for additional information
// regarding copyright ownership.  The ASF licenses this file
// to you under the Apache License, Version 2.0 (the
// "License"); you may not use this file except in compliance
// with the License.  You may obtain a copy of the License at
//
//   http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing,
// software distributed under the License is distributed on an
// "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
// KIND, either express or implied.  See the License for the
// specific language governing permissions and limitations
// under the License.

//! Logic for extracting ObjectMeta from headers used by AWS, GCP and Azure

use crate::path::Path;
use crate::ObjectMeta;
use chrono::{DateTime, TimeZone, Utc};
use http::header::{CONTENT_LENGTH, ETAG, LAST_MODIFIED};
use http::HeaderMap;

#[derive(Debug, Copy, Clone)]
/// Configuration for header extraction
pub(crate) struct HeaderConfig {
    /// Whether to require an ETag header when extracting [`ObjectMeta`] from headers.
    ///
    /// Defaults to `true`
    pub etag_required: bool,

    /// Whether to require a Last-Modified header when extracting [`ObjectMeta`] from headers.
    ///
    /// Defaults to `true`
    pub last_modified_required: bool,

    /// The version header name if any
    pub version_header: Option<&'static str>,

    /// The user defined metadata prefix if any
    pub user_defined_metadata_prefix: Option<&'static str>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("ETag Header missing from response")]
    MissingEtag,

    #[error("Received header containing non-ASCII data")]
    BadHeader { source: reqwest::header::ToStrError },

    #[error("Last-Modified Header missing from response")]
    MissingLastModified,

    #[error("Content-Length Header missing from response")]
    MissingContentLength,

    #[error("Invalid last modified '{}': {}", last_modified, source)]
    InvalidLastModified {
        last_modified: String,
        source: chrono::ParseError,
    },

    #[error("Invalid content length '{}': {}", content_length, source)]
    InvalidContentLength {
        content_length: String,
        source: std::num::ParseIntError,
    },
}

/// Extracts a PutResult from the provided [`HeaderMap`]
#[cfg(any(feature = "aws", feature = "gcp", feature = "azure"))]
pub(crate) fn get_put_result(
    headers: &HeaderMap,
    version: &str,
) -> Result<crate::PutResult, Error> {
    let e_tag = Some(get_etag(headers)?);
    let version = get_version(headers, version)?;
    Ok(crate::PutResult { e_tag, version })
}

/// Extracts a optional version from the provided [`HeaderMap`]
pub(crate) fn get_version(headers: &HeaderMap, version: &str) -> Result<Option<String>, Error> {
    Ok(match headers.get(version) {
        Some(x) => {
            let version = x.to_str().map_err(|source| Error::BadHeader { source })?;
            // S3 reports objects written while bucket versioning is disabled or
            // suspended as the "null version": GETs return the literal "null",
            // while PUT responses omit the header entirely. Treat it as "no
            // version" so PUT- and GET-derived metadata agree. GCS generations
            // and Azure version ids are never the literal "null".
            (version != "null").then(|| version.to_string())
        }
        None => None,
    })
}

/// Extracts an etag from the provided [`HeaderMap`]
pub(crate) fn get_etag(headers: &HeaderMap) -> Result<String, Error> {
    let e_tag = headers.get(ETAG).ok_or(Error::MissingEtag)?;
    Ok(e_tag
        .to_str()
        .map_err(|source| Error::BadHeader { source })?
        .to_string())
}

/// Extracts [`ObjectMeta`] from the provided [`HeaderMap`]
pub(crate) fn header_meta(
    location: &Path,
    headers: &HeaderMap,
    cfg: HeaderConfig,
) -> Result<ObjectMeta, Error> {
    let last_modified = match headers.get(LAST_MODIFIED) {
        Some(last_modified) => {
            let last_modified = last_modified
                .to_str()
                .map_err(|source| Error::BadHeader { source })?;

            DateTime::parse_from_rfc2822(last_modified)
                .map_err(|source| Error::InvalidLastModified {
                    last_modified: last_modified.into(),
                    source,
                })?
                .with_timezone(&Utc)
        }
        None if cfg.last_modified_required => return Err(Error::MissingLastModified),
        None => Utc.timestamp_nanos(0),
    };

    let e_tag = match get_etag(headers) {
        Ok(e_tag) => Some(e_tag),
        Err(Error::MissingEtag) if !cfg.etag_required => None,
        Err(e) => return Err(e),
    };

    let content_length = headers
        .get(CONTENT_LENGTH)
        .ok_or(Error::MissingContentLength)?;

    let content_length = content_length
        .to_str()
        .map_err(|source| Error::BadHeader { source })?;

    let size = content_length
        .parse()
        .map_err(|source| Error::InvalidContentLength {
            content_length: content_length.into(),
            source,
        })?;

    let version = match cfg.version_header {
        Some(header) => get_version(headers, header)?,
        None => None,
    };

    Ok(ObjectMeta {
        location: location.clone(),
        last_modified,
        version,
        size,
        e_tag,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION_HEADER: &str = "x-amz-version-id";

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), v.parse().unwrap()))
            .collect()
    }

    #[test]
    fn test_get_version_normalizes_null() {
        let version = get_version(&headers(&[(VERSION_HEADER, "null")]), VERSION_HEADER).unwrap();
        assert_eq!(version, None);

        let version = get_version(
            &headers(&[(VERSION_HEADER, "3sL4kqtJlcpXro")]),
            VERSION_HEADER,
        )
        .unwrap();
        assert_eq!(version, Some("3sL4kqtJlcpXro".to_string()));

        let version = get_version(&headers(&[]), VERSION_HEADER).unwrap();
        assert_eq!(version, None);
    }

    #[test]
    fn test_header_meta_normalizes_null_version() {
        let cfg = HeaderConfig {
            etag_required: false,
            last_modified_required: false,
            version_header: Some(VERSION_HEADER),
            user_defined_metadata_prefix: None,
        };

        let meta = header_meta(
            &Path::from("foo"),
            &headers(&[("content-length", "0"), (VERSION_HEADER, "null")]),
            cfg,
        )
        .unwrap();
        assert_eq!(meta.version, None);

        let meta = header_meta(
            &Path::from("foo"),
            &headers(&[("content-length", "0"), (VERSION_HEADER, "3sL4kqtJlcpXro")]),
            cfg,
        )
        .unwrap();
        assert_eq!(meta.version, Some("3sL4kqtJlcpXro".to_string()));
    }
}
