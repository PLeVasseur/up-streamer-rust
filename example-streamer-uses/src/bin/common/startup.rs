// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Optional controller-owned startup and source-lifetime barriers for finite roles.

use std::{io::Read, path::Path, time::Duration};
use up_rust::{UCode, UStatus};

/// Completes once before source traffic. Without the explicit controller file,
/// ordinary examples retain their existing startup behavior. No sample warms a
/// publisher or RPC client while waiting for the native peers to become ready.
pub async fn wait_for_start() -> Result<(), UStatus> {
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

/// Names the controller's source-release file for a passive (receiving) role.
pub const RECEIVE_START_ENV: &str = "UPROTOCOL_RECEIVE_START_FILE";

/// Opens a passive role's receive window, once per process. With
/// [`RECEIVE_START_ENV`], waits for the controller's source release (exact
/// marker, same 30 second bound as the source barrier), so controller work
/// after this role reported readiness, such as starting the other peers and
/// stabilization gates, cannot consume a bounded receive window. Frames
/// delivered meanwhile stay queued in the registration; later receives start
/// at once. Ordinary examples omit the variable; the window opens at once.
pub async fn receive_window() -> Result<(), UStatus> {
    static OPENED: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    OPENED
        .get_or_try_init(|| async {
            let Some(path) = std::env::var_os(RECEIVE_START_ENV) else {
                return Ok(());
            };
            let path = Path::new(&path);
            wait_for_marker(path, Duration::from_secs(30), b"START\n", "receive release").await
        })
        .await
        .map(|_| ())
}

async fn wait_for_release(path: &Path, timeout: Duration) -> Result<(), UStatus> {
    wait_for_marker(path, timeout, b"START\n", "source barrier").await
}

/// Retains the finite source endpoint after publication until its controller
/// observes receiver completion. No extra traffic or fixed keepalive delay is used.
/// Ordinary examples omit the variable and keep their existing exit behavior.
pub async fn wait_for_completion() -> Result<(), UStatus> {
    let Some(path) = std::env::var_os("UPROTOCOL_SOURCE_FINISH_FILE") else {
        return Ok(());
    };
    println!("READY source_complete_waiting");
    wait_for_marker(
        Path::new(&path),
        Duration::from_secs(30),
        b"STOP\n",
        "source barrier",
    )
    .await
}

/// `what` names the barrier in errors, so a passive's release wait is not
/// mistaken for the source's start barrier in logs.
async fn wait_for_marker<const N: usize>(
    path: &Path,
    timeout: Duration,
    expected: &[u8; N],
    what: &str,
) -> Result<(), UStatus> {
    let deadline = tokio::time::Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| {
            UStatus::fail_with_code(UCode::InvalidArgument, format!("{what} deadline overflow"))
        })?;
    loop {
        match std::fs::File::open(path) {
            Ok(mut file) => {
                let mut marker = [0u8; N];
                let mut extra = [0u8; 1];
                if file.read_exact(&mut marker).is_err()
                    || &marker != expected
                    || file.read(&mut extra).ok() != Some(0)
                {
                    return Err(UStatus::fail_with_code(
                        UCode::InvalidArgument,
                        format!("invalid {what} marker"),
                    ));
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(UStatus::fail_with_code(
                    UCode::Unavailable,
                    format!("{what} marker: {error}"),
                ))
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(UStatus::fail_with_code(
                UCode::DeadlineExceeded,
                format!("{what} timed out"),
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

    #[tokio::test]
    async fn finite_source_lifetime_requires_the_exact_completion_marker() {
        let marker = Marker::new();
        let waiting = wait_for_marker(
            &marker.0,
            Duration::from_secs(1),
            b"STOP\n",
            "source barrier",
        );
        tokio::pin!(waiting);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), &mut waiting)
                .await
                .is_err()
        );
        std::fs::write(&marker.0, b"STOP\n").unwrap();
        waiting.await.unwrap();
        std::fs::write(&marker.0, b"START\n").unwrap();
        assert_eq!(
            wait_for_marker(
                &marker.0,
                Duration::from_secs(1),
                b"STOP\n",
                "source barrier"
            )
            .await
            .unwrap_err()
            .code(),
            UCode::InvalidArgument
        );
    }
}
