use hotshot::types::SignatureKey;
use hotshot_orchestrator::config::ValidatorConfigFile;
use hotshot_types::traits::election::Membership;
use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use hotshot::traits::{NodeImplementation, TestableNodeImplementation};

use hotshot_types::{
    traits::node_implementation::NodeType, ExecutionType, HotShotConfig, ValidatorConfig,
};

use super::completion_task::{CompletionTaskDescription, TimeBasedCompletionTaskDescription};
use crate::{
    spinning_task::SpinningTaskDescription,
    test_launcher::{ResourceGenerators, TestLauncher},
};

use super::{
    overall_safety_task::OverallSafetyPropertiesDescription, txn_task::TxnTaskDescription,
};
use hotshot::{HotShotType, SystemContext};
/// data describing how a round should be timed.
#[derive(Clone, Debug, Copy)]
pub struct TimingData {
    /// Base duration for next-view timeout, in milliseconds
    pub next_view_timeout: u64,
    /// The exponential backoff ration for the next-view timeout
    pub timeout_ratio: (u64, u64),
    /// The delay a leader inserts before starting pre-commit, in milliseconds
    pub round_start_delay: u64,
    /// Delay after init before starting consensus, in milliseconds
    pub start_delay: u64,
    /// The minimum amount of time a leader has to wait to start a round
    pub propose_min_round_time: Duration,
    /// The maximum amount of time a leader can wait to start a round
    pub propose_max_round_time: Duration,
}

/// metadata describing a test
#[derive(Clone, Debug)]
pub struct TestMetadata {
    /// Total number of nodes in the test
    pub total_nodes: usize,
    /// nodes available at start
    pub start_nodes: usize,
    /// number of bootstrap nodes (libp2p usage only)
    pub num_bootstrap_nodes: usize,
    /// Size of the DA committee for the test
    pub da_committee_size: usize,
    // overall safety property description
    pub overall_safety_properties: OverallSafetyPropertiesDescription,
    /// spinning properties
    pub spinning_properties: SpinningTaskDescription,
    // txns timing
    pub txn_description: TxnTaskDescription,
    // completion task
    pub completion_task_description: CompletionTaskDescription,
    /// Minimum transactions required for a block
    pub min_transactions: usize,
    /// timing data
    pub timing_data: TimingData,
}

impl Default for TimingData {
    fn default() -> Self {
        Self {
            next_view_timeout: 1000,
            timeout_ratio: (11, 10),
            round_start_delay: 100,
            start_delay: 100,
            propose_min_round_time: Duration::new(0, 0),
            propose_max_round_time: Duration::from_millis(100),
        }
    }
}

impl TestMetadata {
    pub fn default_stress() -> Self {
        TestMetadata {
            num_bootstrap_nodes: 15,
            total_nodes: 100,
            start_nodes: 100,
            overall_safety_properties: OverallSafetyPropertiesDescription {
                num_successful_views: 50,
                check_leaf: true,
                check_state: true,
                check_block: true,
                num_failed_views: 15,
                transaction_threshold: 0,
                threshold_calculator: Arc::new(|_active, total| (2 * total / 3 + 1)),
            },
            timing_data: TimingData {
                next_view_timeout: 2000,
                timeout_ratio: (1, 1),
                start_delay: 20000,
                round_start_delay: 25,
                ..TimingData::default()
            },
            ..TestMetadata::default()
        }
    }

    pub fn default_multiple_rounds() -> TestMetadata {
        TestMetadata {
            total_nodes: 10,
            start_nodes: 10,
            overall_safety_properties: OverallSafetyPropertiesDescription {
                num_successful_views: 20,
                check_leaf: true,
                check_state: true,
                check_block: true,
                num_failed_views: 8,
                transaction_threshold: 0,
                threshold_calculator: Arc::new(|_active, total| (2 * total / 3 + 1)),
            },
            timing_data: TimingData {
                start_delay: 120000,
                round_start_delay: 25,
                ..TimingData::default()
            },
            ..TestMetadata::default()
        }
    }

    /// Default setting with 20 nodes and 8 views of successful views.
    pub fn default_more_nodes() -> TestMetadata {
        TestMetadata {
            total_nodes: 20,
            start_nodes: 20,
            num_bootstrap_nodes: 20,
            // The first 14 (i.e., 20 - f) nodes are in the DA committee and we may shutdown the
            // remaining 6 (i.e., f) nodes. We could remove this restriction after fixing the
            // following issue.
            // TODO: Update message broadcasting to avoid hanging
            // <https://github.com/EspressoSystems/HotShot/issues/1567>
            da_committee_size: 14,
            completion_task_description: CompletionTaskDescription::TimeBasedCompletionTaskBuilder(
                TimeBasedCompletionTaskDescription {
                    // Increase the duration to get the expected number of successful views.
                    duration: Duration::new(200, 0),
                },
            ),
            overall_safety_properties: OverallSafetyPropertiesDescription {
                ..Default::default()
            },
            timing_data: TimingData {
                next_view_timeout: 1000,
                ..TimingData::default()
            },
            ..TestMetadata::default()
        }
    }
}

impl Default for TestMetadata {
    /// by default, just a single round
    fn default() -> Self {
        Self {
            timing_data: TimingData::default(),
            min_transactions: 0,
            total_nodes: 5,
            start_nodes: 5,
            num_bootstrap_nodes: 5,
            da_committee_size: 5,
            spinning_properties: SpinningTaskDescription {
                node_changes: vec![],
            },
            overall_safety_properties: OverallSafetyPropertiesDescription::default(),
            // arbitrary, haven't done the math on this
            txn_description: TxnTaskDescription::RoundRobinTimeBased(Duration::from_millis(10)),
            completion_task_description: CompletionTaskDescription::TimeBasedCompletionTaskBuilder(
                TimeBasedCompletionTaskDescription {
                    // TODO ED Put a configurable time here - 10 seconds for now
                    duration: Duration::from_millis(10000),
                },
            ),
        }
    }
}

impl TestMetadata {
    pub fn gen_launcher<TYPES: NodeType, I: TestableNodeImplementation<TYPES>>(
        self,
        node_id: u64,
    ) -> TestLauncher<TYPES, I>
    where
        I: NodeImplementation<TYPES>,
        SystemContext<TYPES, I>: HotShotType<TYPES, I>,
    {
        let TestMetadata {
            total_nodes,
            num_bootstrap_nodes,
            min_transactions,
            timing_data,
            da_committee_size,
            txn_description,
            completion_task_description,
            overall_safety_properties,
            spinning_properties,
            ..
        } = self.clone();

        // We assign known_nodes' public key and stake value here rather than read from config file since it's a test.
        let known_nodes_with_stake = (0..total_nodes)
            .map(|node_id| {
                let cur_validator_config: ValidatorConfig<TYPES::SignatureKey> =
                    ValidatorConfig::generated_from_seed_indexed([0u8; 32], node_id as u64, 1);

                cur_validator_config
                    .public_key
                    .get_stake_table_entry(cur_validator_config.stake_value)
            })
            .collect();
        // But now to test validator's config, we input the info of my_own_validator from config file when node_id == 0.
        let mut my_own_validator_config =
            ValidatorConfig::generated_from_seed_indexed([0u8; 32], node_id, 1);
        if node_id == 0 {
            my_own_validator_config = ValidatorConfig::from(ValidatorConfigFile::from_file(
                "config/ValidatorConfigFile.toml",
            ));
        }
        // let da_committee_nodes = known_nodes[0..da_committee_size].to_vec();
        let config = HotShotConfig {
            // TODO this doesn't exist anymore
            execution_type: ExecutionType::Incremental,
            total_nodes: NonZeroUsize::new(total_nodes).unwrap(),
            num_bootstrap: num_bootstrap_nodes,
            min_transactions,
            max_transactions: NonZeroUsize::new(99999).unwrap(),
            known_nodes_with_stake,
            my_own_validator_config,
            da_committee_size,
            next_view_timeout: 500,
            timeout_ratio: (11, 10),
            round_start_delay: 1,
            start_delay: 1,
            // TODO do we use these fields??
            propose_min_round_time: Duration::from_millis(0),
            propose_max_round_time: Duration::from_millis(1000),
            // TODO what's the difference between this and the second config?
            election_config: Some(TYPES::Membership::default_election_config(
                total_nodes as u64,
            )),
        };
        let TimingData {
            next_view_timeout,
            timeout_ratio,
            round_start_delay,
            start_delay,
            propose_min_round_time,
            propose_max_round_time,
        } = timing_data;
        let mod_config =
            // TODO this should really be using the timing config struct
            |a: &mut HotShotConfig<TYPES::SignatureKey, TYPES::ElectionConfigType>| {
                a.next_view_timeout = next_view_timeout;
                a.timeout_ratio = timeout_ratio;
                a.round_start_delay = round_start_delay;
                a.start_delay = start_delay;
                a.propose_min_round_time = propose_min_round_time;
                a.propose_max_round_time = propose_max_round_time;
            };

        let txn_task_generator = txn_description.build();
        let completion_task_generator = completion_task_description.build_and_launch();
        let overall_safety_task_generator = overall_safety_properties.build();
        let spinning_task_generator = spinning_properties.build();
        TestLauncher {
            resource_generator: ResourceGenerators {
                channel_generator: <I as TestableNodeImplementation<TYPES>>::gen_comm_channels(
                    total_nodes,
                    num_bootstrap_nodes,
                    da_committee_size,
                ),
                storage: Box::new(|_| I::construct_tmp_storage().unwrap()),
                config,
            },
            metadata: self,
            txn_task_generator,
            overall_safety_task_generator,
            completion_task_generator,
            spinning_task_generator,
            hooks: vec![],
        }
        .modify_default_config(mod_config)
    }
}
