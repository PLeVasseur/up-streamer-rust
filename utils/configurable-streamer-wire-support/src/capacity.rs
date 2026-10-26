// Copyright (c) 2026 Contributors to the Eclipse Foundation
// SPDX-License-Identifier: Apache-2.0
//! Explicit deployment capacity choices, applied through the owning binding APIs.
use serde::{Deserialize, Serialize};
use up_rust::{UCode, UStatus};

/// Publisher allocation policy forwarded to iceoryx2.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PublisherAllocationStrategy {
    /// Never grow the configured sample capacity.
    Static,
    /// Grow to the required sample capacity.
    BestFit,
    /// Grow in powers of two (the ordinary binding default).
    PowerOfTwo,
}

impl std::str::FromStr for PublisherAllocationStrategy {
    type Err = String;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "static" => Ok(Self::Static),
            "best_fit" => Ok(Self::BestFit),
            "power_of_two" => Ok(Self::PowerOfTwo),
            _ => Err("expected static, best_fit or power_of_two".into()),
        }
    }
}

impl std::fmt::Display for PublisherAllocationStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Static => "static",
            Self::BestFit => "best_fit",
            Self::PowerOfTwo => "power_of_two",
        })
    }
}

/// Optional additive iceoryx2 capacities; omission preserves binding defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Iceoryx2Capacity {
    /// Initial complete encoded sample slice, including metadata and alignment slack.
    pub publisher_initial_max_slice_len: Option<usize>,
    /// Static or dynamic publisher allocation policy.
    pub publisher_allocation_strategy: Option<PublisherAllocationStrategy>,
    /// Native subscriber ceiling per service, not an application queue limit.
    pub max_subscribers: Option<usize>,
    /// Maximum simultaneous live publisher TX loans.
    pub publisher_max_loaned_samples: Option<usize>,
}

impl Iceoryx2Capacity {
    /// Whether this deployment explicitly selects any capacity overrides.
    pub fn configured(&self) -> bool {
        self.publisher_initial_max_slice_len.is_some()
            || self.publisher_allocation_strategy.is_some()
            || self.max_subscribers.is_some()
            || self.publisher_max_loaned_samples.is_some()
    }

    /// Checks the frontend values before constructing a backend.
    ///
    /// # Errors
    /// Returns `InvalidArgument` for zero capacities. Binding validation also
    /// checks effective slot arithmetic, readiness compatibility and native limits.
    pub fn validate(&self) -> Result<(), UStatus> {
        if [
            self.publisher_initial_max_slice_len,
            self.max_subscribers,
            self.publisher_max_loaned_samples,
        ]
        .contains(&Some(0))
        {
            return Err(UStatus::fail_with_code(
                UCode::InvalidArgument,
                "iceoryx2 capacities must be nonzero",
            ));
        }
        Ok(())
    }

    /// Applies explicit capacities and enables exact binding service validation.
    /// An explicit namespace/native configuration must already be present.
    ///
    /// # Errors
    /// Returns typed invalid/overflow errors from frontend or binding validation.
    #[cfg(any(feature = "iceoryx2-zero-copy", feature = "iceoryx2-owned-frame"))]
    pub fn apply(
        &self,
        config: &mut up_transport_iceoryx2_rust::Iceoryx2PubSubConfig,
    ) -> Result<(), UStatus> {
        self.validate()?;
        if !self.configured() {
            return Ok(());
        }
        let native = config.iceoryx2_config.as_mut().ok_or_else(|| {
            UStatus::fail_with_code(
                UCode::InvalidArgument,
                "iceoryx2 capacity overrides require an explicit namespace",
            )
        })?;
        if let Some(value) = self.publisher_initial_max_slice_len {
            config.publisher_initial_max_slice_len = value;
        }
        if let Some(value) = self.publisher_allocation_strategy {
            config.publisher_allocation_strategy = match value {
                PublisherAllocationStrategy::Static => {
                    iceoryx2::prelude::AllocationStrategy::Static
                }
                PublisherAllocationStrategy::BestFit => {
                    iceoryx2::prelude::AllocationStrategy::BestFit
                }
                PublisherAllocationStrategy::PowerOfTwo => {
                    iceoryx2::prelude::AllocationStrategy::PowerOfTwo
                }
            };
        }
        if let Some(value) = self.max_subscribers {
            native.defaults.publish_subscribe.max_subscribers = value;
        }
        if let Some(value) = self.publisher_max_loaned_samples {
            native
                .defaults
                .publish_subscribe
                .publisher_max_loaned_samples = value;
        }
        config.require_exact_service_limits = true;
        config.validate()
    }
}

#[cfg(all(test, feature = "iceoryx2-zero-copy"))]
mod tests {
    use super::*;
    use up_transport_iceoryx2_rust::Iceoryx2PubSubConfig;

    #[test]
    fn explicit_capacity_reaches_native_config_without_losing_namespace() {
        let mut config = Iceoryx2PubSubConfig::default()
            .with_namespace("/tmp", "capacity_")
            .unwrap();
        let namespace = config.iceoryx2_config.as_ref().unwrap().global.clone();
        let capacity: Iceoryx2Capacity = serde_json::from_str(
            r#"{
            "publisher_initial_max_slice_len": 4096, "publisher_allocation_strategy":"static",
            "max_subscribers":2, "publisher_max_loaned_samples":1}"#,
        )
        .unwrap();
        capacity.apply(&mut config).unwrap();
        assert!(config.require_exact_service_limits);
        assert_eq!(config.publisher_initial_max_slice_len, 4096);
        assert_eq!(
            config.publisher_allocation_strategy,
            iceoryx2::prelude::AllocationStrategy::Static
        );
        let native = config.iceoryx2_config.as_ref().unwrap();
        assert_eq!(native.global, namespace);
        assert_eq!(native.defaults.publish_subscribe.max_subscribers, 2);
        assert_eq!(
            native
                .defaults
                .publish_subscribe
                .publisher_max_loaned_samples,
            1
        );
    }

    #[test]
    fn omitted_limits_preserve_defaults_and_invalid_controls_fail() {
        let mut config = Iceoryx2PubSubConfig::default();
        let before = config.clone();
        Iceoryx2Capacity::default().apply(&mut config).unwrap();
        assert_eq!(config, before);
        let mut overrides = Iceoryx2Capacity {
            max_subscribers: Some(2),
            ..Default::default()
        };
        assert_eq!(
            overrides.apply(&mut config).unwrap_err().code(),
            UCode::InvalidArgument
        );
        config = config.with_namespace("/tmp", "capacity_").unwrap();
        overrides.max_subscribers = Some(0);
        assert_eq!(
            overrides.apply(&mut config).unwrap_err().code(),
            UCode::InvalidArgument
        );
        overrides.max_subscribers = Some(usize::MAX);
        assert_eq!(
            overrides.apply(&mut config).unwrap_err().code(),
            UCode::OutOfRange
        );
        overrides.max_subscribers = Some(1);
        config.publisher_min_subscribers = 2;
        assert_eq!(
            overrides.apply(&mut config).unwrap_err().code(),
            UCode::InvalidArgument
        );
    }
}
