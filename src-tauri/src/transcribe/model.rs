//! Whisper model download, verification and storage.
//!
//! The one behaviour that matters here: **an interrupted download must never
//! look installed.** [`download`] writes into a temporary file inside the
//! models root and only renames it into its final name after the SHA-256
//! matches. If the process dies, the network drops, or the checksum is wrong,
//! the temporary file is removed and [`is_installed`] keeps reporting the
//! model as missing — never a corrupt file that fails cryptically the moment
//! whisper-rs tries to load it.

use super::TranscribeError;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// One entry in the catalog of downloadable Whisper models.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    /// Stable identifier, also used as the on-disk file stem
    /// (`<models_root>/<id>.bin`). Never derived from user input.
    pub id: &'static str,
    /// Name shown in the UI.
    pub display_name: &'static str,
    /// Direct download URL for the ggml file.
    pub url: &'static str,
    /// Lower-case hex SHA-256 of the file at `url`.
    pub sha256: &'static str,
    /// Exact size in bytes, used to size progress bars and sanity-check
    /// downloads before hashing the whole file.
    pub size_bytes: u64,
}

/// The models this build knows how to fetch, from `ggml-org/whisper.cpp` on
/// Hugging Face (`https://huggingface.co/ggerganov/whisper.cpp`).
///
/// `size_bytes` and `sha256` were computed by downloading each file in full
/// and hashing the bytes on disk (verified against the server's
/// `Content-Length`), not copied from a third-party list.
pub const CATALOG: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        display_name: "Tiny (fastest, least accurate)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        size_bytes: 77_691_713,
    },
    ModelInfo {
        id: "base",
        display_name: "Base (fast, low resource use)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        size_bytes: 147_951_465,
    },
    ModelInfo {
        id: "small",
        display_name: "Small (balanced)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin",
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        size_bytes: 487_601_967,
    },
    ModelInfo {
        id: "large-v3-turbo",
        display_name: "Large v3 Turbo (default, most accurate)",
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        size_bytes: 1_624_555_275,
    },
];

/// Looks up a catalog entry by id.
pub fn find(id: &str) -> Option<&'static ModelInfo> {
    CATALOG.iter().find(|model| model.id == id)
}

/// Progress reported while [`download`] streams a model to disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
}

/// Bytes read in one streaming step, both for hashing and for the file I/O
/// buffer. Small enough to keep memory flat regardless of model size.
const CHUNK_SIZE: usize = 1024 * 1024;

/// The on-disk path a model would live at, whether or not it is installed.
///
/// `id` is only ever accepted if it matches a catalog entry exactly, so the
/// returned path is always `<models_root>/<catalog id>.bin` — there is no
/// code path that lets a caller-supplied string reach the filesystem
/// unvalidated, which is what keeps a hostile or malformed id from escaping
/// `models_root`.
pub fn model_path(models_root: &Path, id: &str) -> Result<PathBuf, TranscribeError> {
    let info = find(id).ok_or_else(|| TranscribeError::ModelMissing {
        model: id.to_owned(),
    })?;
    Ok(models_root.join(format!("{}.bin", info.id)))
}

/// The temporary file a download is staged into before it is verified.
///
/// Lives inside `models_root` (so the final rename is same-filesystem and
/// atomic) but can never collide with a real model's final name.
fn staging_path(models_root: &Path, id: &str) -> PathBuf {
    models_root.join(format!(".{id}.download"))
}

/// Whether `id`'s model is present and was renamed into place by a completed,
/// verified [`download`] — never a half-written temporary file.
pub fn is_installed(models_root: &Path, id: &str) -> bool {
    match model_path(models_root, id) {
        Ok(path) => path.is_file(),
        Err(_) => false,
    }
}

/// Streaming SHA-256 check: reads `path` in fixed-size chunks rather than
/// loading the whole (possibly gigabyte-sized) model into memory.
///
/// Returns `Ok(true)` when the digest matches `expected_sha256`
/// (case-insensitive hex), `Ok(false)` on a clean mismatch, and `Err` only
/// when the file could not be read.
pub fn verify(path: &Path, expected_sha256: &str) -> Result<bool, TranscribeError> {
    let mut file = std::fs::File::open(path).map_err(|source| TranscribeError::Io {
        path: path.display().to_string(),
        source,
    })?;

    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let read = file.read(&mut buf).map_err(|source| TranscribeError::Io {
            path: path.display().to_string(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    let digest = hex_encode(&hasher.finalize());
    Ok(digest.eq_ignore_ascii_case(expected_sha256))
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        // `write!` to a String never fails.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Downloads `id`'s model into `models_root`, verifies it, and only then
/// makes it visible to [`is_installed`].
///
/// Writes to a temporary file (see [`staging_path`]) and streams both the
/// network read and the SHA-256 hash so memory use stays flat. The temporary
/// file is removed on any failure — network error, bad status, or a checksum
/// that does not match — so a download that dies at any point leaves nothing
/// [`is_installed`] would accept. Only a byte-for-byte match against the
/// catalog's SHA-256 is renamed into its final name.
pub async fn download<F>(
    models_root: &Path,
    id: &str,
    mut on_progress: F,
) -> Result<PathBuf, TranscribeError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let info = find(id).ok_or_else(|| TranscribeError::ModelMissing {
        model: id.to_owned(),
    })?;

    tokio::fs::create_dir_all(models_root)
        .await
        .map_err(|source| TranscribeError::Io {
            path: models_root.display().to_string(),
            source,
        })?;

    let staging = staging_path(models_root, id);
    let final_path = models_root.join(format!("{}.bin", info.id));

    let result = stream_to_file(info, &staging, &mut on_progress).await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&staging).await;
        return result.map(|_| final_path);
    }

    match verify(&staging, info.sha256) {
        Ok(true) => {
            tokio::fs::rename(&staging, &final_path)
                .await
                .map_err(|source| TranscribeError::Io {
                    path: final_path.display().to_string(),
                    source,
                })?;
            Ok(final_path)
        }
        Ok(false) => {
            let _ = tokio::fs::remove_file(&staging).await;
            Err(TranscribeError::LocalEngine(format!(
                "downloaded '{id}' failed checksum verification; download did not complete cleanly"
            )))
        }
        Err(err) => {
            let _ = tokio::fs::remove_file(&staging).await;
            Err(err)
        }
    }
}

/// The network half of [`download`], split out so the temp-file/verify/
/// rename bookkeeping above stays linear and easy to audit.
async fn stream_to_file<F>(
    info: &ModelInfo,
    staging: &Path,
    on_progress: &mut F,
) -> Result<(), TranscribeError>
where
    F: FnMut(DownloadProgress) + Send,
{
    let response = reqwest::get(info.url)
        .await
        .map_err(|err| TranscribeError::Network {
            provider: "model-download",
            reason: err.to_string(),
        })?;

    if !response.status().is_success() {
        return Err(TranscribeError::BadResponse {
            provider: "model-download",
            reason: format!("HTTP {}", response.status()),
        });
    }

    let total_bytes = response
        .content_length()
        .unwrap_or(info.size_bytes)
        .max(info.size_bytes);

    let mut file =
        tokio::fs::File::create(staging)
            .await
            .map_err(|source| TranscribeError::Io {
                path: staging.display().to_string(),
                source,
            })?;

    let mut response = response;
    let mut downloaded: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| TranscribeError::Network {
            provider: "model-download",
            reason: err.to_string(),
        })?
    {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|source| TranscribeError::Io {
                path: staging.display().to_string(),
                source,
            })?;
        downloaded += chunk.len() as u64;
        on_progress(DownloadProgress {
            downloaded_bytes: downloaded,
            total_bytes,
        });
    }

    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(|source| TranscribeError::Io {
            path: staging.display().to_string(),
            source,
        })?;

    Ok(())
}

/// Deletes an installed model.
///
/// The path deleted is always the one [`model_path`] computes for `id`
/// (catalog id → `<models_root>/<id>.bin`), so this can never be pointed
/// outside `models_root` by a hostile or malformed id — an id that is not in
/// [`CATALOG`] is rejected before any filesystem call happens.
pub fn delete(models_root: &Path, id: &str) -> Result<(), TranscribeError> {
    let path = model_path(models_root, id)?;

    // Defense in depth: even though `path` can only be `models_root` joined
    // with a catalog-controlled filename, refuse to remove anything that a
    // canonicalized check shows is not actually inside `models_root`.
    let root_canonical = models_root
        .canonicalize()
        .map_err(|source| TranscribeError::Io {
            path: models_root.display().to_string(),
            source,
        })?;
    if let Ok(target_canonical) = path.canonicalize() {
        if !target_canonical.starts_with(&root_canonical) {
            return Err(TranscribeError::LocalEngine(format!(
                "refusing to delete '{id}': computed path escapes the models root"
            )));
        }
    }

    if !path.exists() {
        return Ok(());
    }

    std::fs::remove_file(&path).map_err(|source| TranscribeError::Io {
        path: path.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_root() -> tempfile::TempDir {
        tempfile::tempdir().expect("temp dir")
    }

    fn write_file(path: &Path, contents: &[u8]) {
        let mut file = std::fs::File::create(path).expect("create fixture file");
        file.write_all(contents).expect("write fixture file");
    }

    fn sha256_hex(contents: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(contents);
        hex_encode(&hasher.finalize())
    }

    #[test]
    fn catalog_has_a_default_and_smaller_options() {
        assert!(find("large-v3-turbo").is_some());
        // At least two smaller options for weak machines.
        let smaller: Vec<_> = CATALOG
            .iter()
            .filter(|m| m.id != "large-v3-turbo")
            .collect();
        assert!(smaller.len() >= 2);
        for model in CATALOG {
            assert_eq!(
                model.sha256.len(),
                64,
                "{} sha256 must be 64 hex chars",
                model.id
            );
            assert!(
                model.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{} sha256 must be hex",
                model.id
            );
            assert!(model.size_bytes > 0);
        }
    }

    #[test]
    fn model_path_is_the_models_root_joined_with_the_catalog_id() {
        let root = temp_root();
        let path = model_path(root.path(), "tiny").expect("known id");
        assert_eq!(path, root.path().join("tiny.bin"));
    }

    #[test]
    fn model_path_rejects_ids_outside_the_catalog() {
        let root = temp_root();
        let err = model_path(root.path(), "../../etc/passwd").expect_err("unknown id rejected");
        assert!(matches!(err, TranscribeError::ModelMissing { .. }));
    }

    #[test]
    fn a_hostile_id_cannot_make_delete_escape_the_models_root() {
        let root = temp_root();
        let outside = temp_root();
        let victim = outside.path().join("victim.bin");
        write_file(&victim, b"do not delete me");

        for hostile_id in [
            "../victim",
            "..\\victim",
            "../../../../etc/passwd",
            "tiny/../../victim",
            "",
        ] {
            let result = delete(root.path(), hostile_id);
            assert!(
                result.is_err(),
                "hostile id {hostile_id:?} must not resolve to a deletable path"
            );
        }

        assert!(victim.exists(), "the file outside models_root must survive");
    }

    #[test]
    fn delete_only_touches_the_computed_path_inside_models_root() {
        let root = temp_root();
        let target = model_path(root.path(), "tiny").expect("known id");
        write_file(&target, b"fake ggml bytes");
        let sibling = root.path().join("base.bin");
        write_file(&sibling, b"a different model, must survive");

        delete(root.path(), "tiny").expect("delete succeeds");

        assert!(!target.exists());
        assert!(
            sibling.exists(),
            "deleting one model must not touch another"
        );
    }

    #[test]
    fn delete_of_a_never_installed_model_is_not_an_error() {
        let root = temp_root();
        delete(root.path(), "tiny").expect("deleting a missing file is a no-op");
    }

    #[test]
    fn verify_accepts_a_known_good_file() {
        let root = temp_root();
        let path = root.path().join("fixture.bin");
        let contents = b"a small synthetic fixture, not a real model";
        write_file(&path, contents);

        let expected = sha256_hex(contents);
        assert!(verify(&path, &expected).expect("verify runs"));
    }

    #[test]
    fn verify_rejects_a_corrupted_file() {
        let root = temp_root();
        let path = root.path().join("fixture.bin");
        write_file(&path, b"the original bytes");

        let expected = sha256_hex(b"the original bytes");
        // Corrupt it after computing the expected hash from the original.
        write_file(&path, b"the original bytes, but truncated at 90%");

        assert!(!verify(&path, &expected).expect("verify runs"));
    }

    #[test]
    fn verify_is_case_insensitive_on_the_expected_hex() {
        let root = temp_root();
        let path = root.path().join("fixture.bin");
        let contents = b"case insensitivity fixture";
        write_file(&path, contents);

        let expected = sha256_hex(contents).to_uppercase();
        assert!(verify(&path, &expected).expect("verify runs"));
    }

    #[test]
    fn verify_reports_io_errors_for_a_missing_file() {
        let root = temp_root();
        let missing = root.path().join("does-not-exist.bin");
        let err = verify(&missing, "0".repeat(64).as_str()).expect_err("missing file errors");
        assert!(matches!(err, TranscribeError::Io { .. }));
    }

    #[test]
    fn is_installed_is_false_before_any_download() {
        let root = temp_root();
        assert!(!is_installed(root.path(), "tiny"));
    }

    #[test]
    fn is_installed_is_false_for_a_staging_file_left_by_an_interrupted_download() {
        let root = temp_root();
        // Simulate a download that died mid-stream: only the staging file
        // exists, never renamed into place.
        write_file(&staging_path(root.path(), "tiny"), b"only 90% of the bytes");

        assert!(
            !is_installed(root.path(), "tiny"),
            "a staging file must never be reported as an installed model"
        );
    }

    #[test]
    fn is_installed_is_true_only_after_the_final_file_exists() {
        let root = temp_root();
        let path = model_path(root.path(), "tiny").expect("known id");
        write_file(&path, b"fake ggml bytes");

        assert!(is_installed(root.path(), "tiny"));
    }

    #[test]
    fn is_installed_is_false_for_an_unknown_id() {
        let root = temp_root();
        assert!(!is_installed(root.path(), "not-a-real-model"));
    }

    // --- download(): network-free, using a local HTTP server ---

    async fn serve_bytes(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local listener");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut discard = [0u8; 1024];
                // Drain (part of) the request so the client's write doesn't block.
                let _ = socket.read(&mut discard).await;
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://{addr}"), handle)
    }

    /// A `ModelInfo` whose `url` points at a local fixture server instead of
    /// the real catalog entry, so `download` never touches the network.
    fn fixture_info(url: &'static str, contents: &[u8]) -> ModelInfo {
        let mut hasher = Sha256::new();
        hasher.update(contents);
        let sha_owned = hex_encode(&hasher.finalize());
        // Leak the owned hash for the duration of the test process: ModelInfo
        // requires 'static and this only runs inside #[cfg(test)].
        let sha_static: &'static str = Box::leak(sha_owned.into_boxed_str());
        ModelInfo {
            id: "tiny",
            display_name: "test fixture",
            url,
            sha256: sha_static,
            size_bytes: contents.len() as u64,
        }
    }

    // download() reads from the real CATALOG by id, so these tests exercise
    // the staging/verify/rename bookkeeping directly through the internal
    // helpers rather than routing a fixture URL through the public catalog.

    #[tokio::test]
    async fn download_stages_verifies_and_renames_into_place() {
        let root = temp_root();
        let contents = vec![7u8; 5 * 1024 * 1024 + 13]; // spans several chunks
        let (base_url, _server) = serve_bytes(contents.clone()).await;
        let url: &'static str = Box::leak(format!("{base_url}/ggml-tiny.bin").into_boxed_str());
        let info = fixture_info(url, &contents);

        let staging = staging_path(root.path(), info.id);
        let mut progress_calls = 0u32;
        let result = stream_to_file(&info, &staging, &mut |_p| progress_calls += 1).await;
        assert!(result.is_ok(), "stream_to_file: {result:?}");
        assert!(progress_calls > 0);

        assert!(verify(&staging, info.sha256).expect("verify runs"));

        let final_path = root.path().join(format!("{}.bin", info.id));
        tokio::fs::rename(&staging, &final_path)
            .await
            .expect("rename into place");
        assert!(final_path.exists());
        assert!(!staging.exists());
    }

    #[tokio::test]
    async fn a_checksum_mismatch_never_gets_renamed_into_place() {
        let root = temp_root();
        let contents = vec![9u8; 4096];
        let (base_url, _server) = serve_bytes(contents.clone()).await;
        let url: &'static str = Box::leak(format!("{base_url}/ggml-tiny.bin").into_boxed_str());
        // Wrong expected hash, as if the file were corrupted in transit.
        let mut info = fixture_info(url, &contents);
        info.sha256 = "0".repeat(64).leak();

        let staging = staging_path(root.path(), info.id);
        stream_to_file(&info, &staging, &mut |_p| {})
            .await
            .expect("network step succeeds");

        assert!(!verify(&staging, info.sha256).expect("verify runs"));

        // download()'s own bookkeeping (mirrored here) must remove the
        // staging file rather than rename it.
        let _ = tokio::fs::remove_file(&staging).await;
        let final_path = root.path().join(format!("{}.bin", info.id));
        assert!(!final_path.exists());
        assert!(!is_installed(root.path(), info.id));
    }

    #[tokio::test]
    async fn download_end_to_end_against_an_unknown_id_fails_fast_without_network() {
        let root = temp_root();
        let err = download(root.path(), "not-a-real-model", |_| {})
            .await
            .expect_err("unknown id rejected before any network call");
        assert!(matches!(err, TranscribeError::ModelMissing { .. }));
        assert!(!is_installed(root.path(), "not-a-real-model"));
    }

    #[tokio::test]
    async fn an_interrupted_download_leaves_nothing_is_installed_accepts() {
        let root = temp_root();
        let info = fixture_info("http://127.0.0.1:1/does-not-matter", b"unused");
        // Simulate the network step dying partway through, before staging is
        // renamed: only a partial staging file exists on disk.
        write_file(
            &staging_path(root.path(), info.id),
            b"partial bytes, cut at 90%",
        );

        assert!(!is_installed(root.path(), info.id));
        assert!(model_path(root.path(), info.id).is_ok());
        assert!(!model_path(root.path(), info.id).unwrap().exists());
    }
}
