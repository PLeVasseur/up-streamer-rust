// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Optional controller-owned first-send barrier for finite connectivity roles.
#![allow(dead_code)]

use std::{io::Read, path::Path, time::Duration};
use up_rust::{UCode, UStatus};

/// Completes once before source traffic. Without the explicit controller file,
/// ordinary examples retain their existing startup behavior. No sample warms a
/// publisher or RPC client while waiting for the native peers to become ready.
pub(crate) async fn wait_for_start() -> Result<(), UStatus> {
    static STARTED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    STARTED
        .get_or_try_init(|| async {
            let Some(path) = std::env::var_os("UPROTOCOL_SOURCE_START_FILE") else {
                return Ok(());
            };
            println!("READY source_waiting");
            wait_for_release(Path::new(&path), Duration::from_secs(30)).await
        })
        .await
        .map(|_| ())
}

async fn wait_for_release(path: &Path, timeout: Duration) -> Result<(), UStatus> {
    let deadline = tokio::time::Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| {
            UStatus::fail_with_code(UCode::InvalidArgument, "source start deadline overflow")
        })?;
    loop {
        match std::fs::File::open(path) {
            Ok(mut file) => {
                let mut marker = [0u8; 6];
                let mut extra = [0u8; 1];
                if file.read_exact(&mut marker).is_err()
                    || &marker != b"START\n"
                    || file.read(&mut extra).ok() != Some(0)
                {
                    return Err(UStatus::fail_with_code(
                        UCode::InvalidArgument,
                        "invalid source start marker",
                    ));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(UStatus::fail_with_code(
                    UCode::Unavailable,
                    format!("source start marker: {error}"),
                ))
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                "source start barrier timed out",
            ));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Marker(std::path::PathBuf);
    impl Marker {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "source-start-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            assert!(!path.exists());
            Self(path)
        }
    }
    impl Drop for Marker {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn source_waits_until_controller_releases_first_send() {
        let marker = Marker::new();
        let wait = wait_for_release(&marker.0, Duration::from_secs(1));
        tokio::pin!(wait);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut wait)
            .await
            .is_err());
        std::fs::write(&marker.0, b"START\n").unwrap();
        wait.await.unwrap();
    }

    #[tokio::test]
    async fn missing_or_malformed_release_never_starts_source() {
        let marker = Marker::new();
        let error = wait_for_release(&marker.0, Duration::from_millis(1))
            .await
            .unwrap_err();
        assert_eq!(error.code(), UCode::DeadlineExceeded);
        std::fs::write(&marker.0, b"START\ntrailing").unwrap();
        let error = wait_for_release(&marker.0, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(error.code(), UCode::InvalidArgument);
    }
}
