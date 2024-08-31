//! PayTube. A simple SPL payment channel.
//!
//! PayTube is an SVM-based payment channel that allows two parties to exchange
//! tokens off-chain. The channel is opened by invoking the PayTube "VM",
//! running on some arbitrary server(s). When transacting has concluded, the
//! channel is closed by submitting the final payment ledger to Solana.
//!
//! The final ledger tracks debits and credits to all registered token accounts
//! or system accounts (native SOL) during the lifetime of a channel. It is
//! then used to to craft a batch of transactions to submit to the settlement
//! chain (Solana).
//!
//! Users opt-in to using a PayTube channel by "registering" their token
//! accounts to the channel. This is done by delegating a token account to the
//! PayTube on-chain program on Solana. This delegation is temporary, and
//! released immediately after channel settlement.
//!
//! Note: This opt-in solution is for demonstration purposes only.
//!
//! ```text
//!
//! PayTube "VM"
//!
//!    Bob          Alice        Bob          Alice          Will
//!     |             |           |             |             |
//!     | --o--o--o-> |           | --o--o--o-> |             |
//!     |             |           |             | --o--o--o-> | <--- PayTube
//!     | <-o--o--o-- |           | <-o--o--o-- |             |    Transactions
//!     |             |           |             |             |
//!     | --o--o--o-> |           |     -----o--o--o----->    |
//!     |             |           |                           |
//!     | --o--o--o-> |           |     <----o--o--o------    |
//!
//!       \        /                  \         |         /
//!
//!         ------                           ------
//!        Alice: x                         Alice: x
//!        Bob:   x                         Bob:   x    <--- Solana Transaction
//!                                         Will:  x         with final ledgers
//!         ------                           ------
//!
//!           \\                               \\
//!            x                                x
//!
//!         Solana                           Solana     <--- Settled to Solana
//! ```
//!
//! The Solana SVM's `TransactionBatchProcessor` requires projects to provide a
//! "loader" plugin, which implements the `TransactionProcessingCallback`
//! interface.
//!
//! PayTube defines a `PayTubeAccountLoader` that implements the
//! `TransactionProcessingCallback` interface, and provides it to the
//! `TransactionBatchProcessor` to process PayTube transactions.

mod loader;
mod log;
mod processor;
mod settler;
pub mod transaction;
pub mod transaction_builder;
use {
    crate::{
        loader::{PayTubeAccountLoader, PayTubeAccountLoaderWithLocalDB},
        settler::PayTubeSettler,
        transaction::PayTubeTransaction,
    },
    loader::deploy_program,
    processor::{
        create_transaction_batch_processor, get_transaction_check_results, PayTubeForkGraph,
    },
    solana_client::rpc_client::RpcClient,
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_sdk::{
        feature_set::FeatureSet,
        fee::FeeStructure,
        hash::Hash,
        pubkey::Pubkey,
        rent_collector::RentCollector,
        signature::{Keypair, Signature},
    },
    solana_svm::{
        transaction_processing_callback::TransactionProcessingCallback,
        transaction_processing_result::TransactionProcessingResultExtensions,
        transaction_processor::{
            ExecutionRecordingConfig, TransactionProcessingConfig, TransactionProcessingEnvironment,
        },
    },
    std::{
        collections::HashMap,
        sync::{Arc, RwLock},
    },
    transaction::create_svm_transactions,
    transaction_builder::SanitizedTransactionBuilder,
};

/// A PayTube channel instance.
///
/// Facilitates native SOL or SPL token transfers amongst various channel
/// participants, settling the final changes in balances to the base chain.
pub struct PayTubeChannel {
    /// I think you know why this is a bad idea...
    keys: Vec<Keypair>,
    rpc_client: RpcClient,
}

impl PayTubeChannel {
    pub fn new(keys: Vec<Keypair>, rpc_client: RpcClient) -> Self {
        Self { keys, rpc_client }
    }

    /// The PayTube API. Processes a batch of PayTube transactions.
    ///
    /// Obviously this is a very simple implementation, but one could imagine
    /// a more complex service that employs custom functionality, such as:
    ///
    /// * Increased throughput for individual P2P transfers.
    /// * Custom Solana transaction ordering (e.g. MEV).
    ///
    /// The general scaffold of the PayTube API would remain the same.
    pub fn process_paytube_transfers(
        &self,
        transactions: &[PayTubeTransaction],
        fee_payer: Pubkey,
    ) {
        log::setup_solana_logging();
        log::creating_paytube_channel();

        // PayTube default configs.
        //
        // These can be configurable for channel customization, including
        // imposing resource or feature restrictions, but more commonly they
        // would likely be hoisted from the cluster.
        //
        // For example purposes, they are provided as defaults here.
        let compute_budget = ComputeBudget::default();
        let feature_set = FeatureSet::all_enabled(); // @todo feature set is like enabling all the availble fetures that are equivalent or similar to precompiles in ethereum.
        let fee_structure = FeeStructure::default();
        let lamports_per_signature = fee_structure.lamports_per_signature;
        let rent_collector = RentCollector::default();

        // PayTube loader/callback implementation.
        //
        // Required to provide the SVM API with a mechanism for loading
        // accounts.
        let account_loader = PayTubeAccountLoader::new(&self.rpc_client);
        account_loader.get_account_shared_data(&spl_token::id());
        // Solana SVM transaction batch processor.
        //
        // Creates an instance of `TransactionBatchProcessor`, which can be
        // used by PayTube to process transactions using the SVM.
        //
        // This allows programs such as the System and Token programs to be
        // translated and executed within a provisioned virtual machine, as
        // well as offers many of the same functionality as the lower-level
        // Solana runtime.
        let fork_graph = Arc::new(RwLock::new(PayTubeForkGraph {}));
        let processor = create_transaction_batch_processor(
            &account_loader,
            &feature_set,
            &compute_budget,
            Arc::clone(&fork_graph),
        );

        // The PayTube transaction processing runtime environment.
        //
        // Again, these can be configurable or hoisted from the cluster.
        let processing_environment = TransactionProcessingEnvironment {
            blockhash: Hash::default(),
            epoch_total_stake: None,
            epoch_vote_accounts: None,
            feature_set: Arc::new(feature_set),
            fee_structure: Some(&fee_structure),
            lamports_per_signature,
            rent_collector: Some(&rent_collector),
        };

        // The PayTube transaction processing config for Solana SVM.
        //
        // Extended configurations for even more customization of the SVM API.
        // @todo additional configuratinos can be done here.
        let processing_config = TransactionProcessingConfig {
            compute_budget: Some(compute_budget),
            recording_config: ExecutionRecordingConfig {
                enable_log_recording: true,
                enable_return_data_recording: true,
                enable_cpi_recording: false,
            },
            ..Default::default()
        };

        // Step 1: Convert the batch of PayTube transactions into
        // SVM-compatible transactions for processing.
        //
        // In the future, the SVM API may allow for trait-based transactions.
        // In this case, `PayTubeTransaction` could simply implement the
        // interface, and avoid this conversion entirely.
        let mut svm_transactions = create_svm_transactions(transactions);
        // @todo we need solana sanitzed transactions, that needed to be sequenced for execution.
        // @todo deploy contract
        let hello_program = deploy_program(String::from("hello-solana"), 0, &account_loader);

        let mut transaction_builder = SanitizedTransactionBuilder::default();
        transaction_builder.create_instruction(
            hello_program,
            Vec::new(),
            HashMap::new(),
            Vec::new(),
        );

        let sanitized_transaction = transaction_builder
            .build(Hash::default(), (fee_payer, Signature::new_unique()), false)
            .unwrap();
        svm_transactions.push(sanitized_transaction);
        // Step 2: Process the SVM-compatible transactions with the SVM API.
        log::processing_transactions(svm_transactions.len());
        println!(
            "program cache: {:?}\n\n",
            processor.program_cache.read().unwrap()
        );
        let results = processor.load_and_execute_sanitized_transactions(
            &account_loader,
            &svm_transactions,
            get_transaction_check_results(svm_transactions.len() + 1, lamports_per_signature),
            &processing_environment,
            &processing_config,
        );
        println!(
            "program cache post exec: {:?}",
            processor.program_cache.read().unwrap()
        );
        println!("results: {:?}", results.processing_results);
        // Step 3: Convert the SVM API processor results into a final ledger
        // using `PayTubeSettler`, and settle the resulting balance differences
        // to the Solana base chain.
        //
        // Here the settler is basically iterating over the transaction results
        // to track debits and credits, but only for those transactions which
        // were executed succesfully.
        //
        // The final ledger of debits and credits to each participant can then
        // be packaged into a minimal number of settlement transactions for
        // submission.
        let executed_tx_0 = results.processing_results[4]
            .processed_transaction()
            .unwrap();
        assert!(executed_tx_0.was_successful());
        let logs = executed_tx_0
            .execution_details
            .log_messages
            .as_ref()
            .unwrap();
        assert!(logs.contains(&"Program log: Hello, Solana!".to_string()));
        // @todo implement the logger. and understand contract deployment.
        let settler = PayTubeSettler::new(&self.rpc_client, transactions, results, &self.keys);
        log::settling_to_base_chain(settler.num_transactions());
        settler.process_settle();

        log::channel_closed();
    }
}

// pub struct PayTubeChannelWithDB {}

// impl PayTubeChannelWithDB {
//     pub fn new() -> Self {
//         Self {}
//     }

//     /// The PayTube API. Processes a batch of PayTube transactions.
//     ///
//     /// Obviously this is a very simple implementation, but one could imagine
//     /// a more complex service that employs custom functionality, such as:
//     ///
//     /// * Increased throughput for individual P2P transfers.
//     /// * Custom Solana transaction ordering (e.g. MEV).
//     ///
//     /// The general scaffold of the PayTube API would remain the same.
//     pub fn process_paytube_transfers(&self, transactions: &[PayTubeTransaction]) {
//         log::setup_solana_logging();
//         log::creating_paytube_channel();

//         // PayTube default configs.
//         //
//         // These can be configurable for channel customization, including
//         // imposing resource or feature restrictions, but more commonly they
//         // would likely be hoisted from the cluster.
//         //
//         // For example purposes, they are provided as defaults here.
//         let compute_budget = ComputeBudget::default();
//         let feature_set = FeatureSet::all_enabled();
//         let fee_structure = FeeStructure::default();
//         let lamports_per_signature = fee_structure.lamports_per_signature;
//         let rent_collector = RentCollector::default();

//         // PayTube loader/callback implementation.
//         //
//         // Required to provide the SVM API with a mechanism for loading
//         // accounts.
//         let mut account_loader = PayTubeAccountLoaderWithLocalDB::new();

//         // Solana SVM transaction batch processor.
//         //
//         // Creates an instance of `TransactionBatchProcessor`, which can be
//         // used by PayTube to process transactions using the SVM.
//         //
//         // This allows programs such as the System and Token programs to be
//         // translated and executed within a provisioned virtual machine, as
//         // well as offers many of the same functionality as the lower-level
//         // Solana runtime.
//         let fork_graph = Arc::new(RwLock::new(PayTubeForkGraph {}));
//         let processor = create_transaction_batch_processor(
//             &mut account_loader,
//             &feature_set,
//             &compute_budget,
//             Arc::clone(&fork_graph),
//         );

//         // The PayTube transaction processing runtime environment.
//         //
//         // Again, these can be configurable or hoisted from the cluster.
//         let processing_environment = TransactionProcessingEnvironment {
//             blockhash: Hash::default(),
//             epoch_total_stake: None,
//             epoch_vote_accounts: None,
//             feature_set: Arc::new(feature_set),
//             fee_structure: Some(&fee_structure),
//             lamports_per_signature,
//             rent_collector: Some(&rent_collector),
//         };

//         // The PayTube transaction processing config for Solana SVM.
//         //
//         // Extended configurations for even more customization of the SVM API.
//         let processing_config = TransactionProcessingConfig {
//             compute_budget: Some(compute_budget),
//             ..Default::default()
//         };

//         // Step 1: Convert the batch of PayTube transactions into
//         // SVM-compatible transactions for processing.
//         //
//         // In the future, the SVM API may allow for trait-based transactions.
//         // In this case, `PayTubeTransaction` could simply implement the
//         // interface, and avoid this conversion entirely.
//         let svm_transactions = create_svm_transactions(transactions);

//         // Step 2: Process the SVM-compatible transactions with the SVM API.
//         log::processing_transactions(svm_transactions.len());
//         let results = processor.load_and_execute_sanitized_transactions(
//             &account_loader,
//             &svm_transactions,
//             get_transaction_check_results(svm_transactions.len(), lamports_per_signature), // @todo we are mocking signatures too, here.
//             &processing_environment,
//             &processing_config,
//         );
//         println!("results: {:?}", results.processing_results);
//         // Step 3: Convert the SVM API processor results into a final ledger
//         // using `PayTubeSettler`, and settle the resulting balance differences
//         // to the Solana base chain.
//         //
//         // Here the settler is basically iterating over the transaction results
//         // to track debits and credits, but only for those transactions which
//         // were executed succesfully.
//         //
//         // The final ledger of debits and credits to each participant can then
//         // be packaged into a minimal number of settlement transactions for
//         // submission.
//         let settler = PayTubeSettler::new(&self.rpc_client, transactions, results, &self.keys);
//         log::settling_to_base_chain(settler.num_transactions());
//         settler.process_settle();

//         log::channel_closed();
//     }
// }
