//! A helper to initialize Solana SVM API's `TransactionBatchProcessor`.

use {
    solana_bpf_loader_program::syscalls::{create_program_runtime_environment_v1, SyscallLog},
    solana_compute_budget::compute_budget::ComputeBudget,
    solana_program_runtime::{
        invoke_context::InvokeContext,
        loaded_programs::{BlockRelation, ForkGraph, LoadProgramMetrics, ProgramCacheEntry},
        solana_rbpf::program::BuiltinProgram,
    },
    solana_sdk::{
        account::ReadableAccount, bpf_loader_upgradeable, clock::Slot, feature_set::FeatureSet,
        transaction,
    },
    solana_svm::{
        account_loader::CheckedTransactionDetails,
        transaction_processing_callback::TransactionProcessingCallback,
        transaction_processor::TransactionBatchProcessor,
    },
    solana_system_program::system_processor,
    std::{
        collections::HashSet,
        sync::{Arc, RwLock},
    },
};

/// In order to use the `TransactionBatchProcessor`, another trait - Solana
/// Program Runtime's `ForkGraph` - must be implemented, to tell the batch
/// processor how to work across forks.
///
/// Since PayTube doesn't use slots or forks, this implementation is mocked.
pub(crate) struct PayTubeForkGraph {}

impl ForkGraph for PayTubeForkGraph {
    fn relationship(&self, _a: Slot, _b: Slot) -> BlockRelation {
        BlockRelation::Unknown
    }
}

/// This function encapsulates some initial setup required to tweak the
/// `TransactionBatchProcessor` for use within PayTube.
///
/// We're simply configuring the mocked fork graph on the SVM API's program
/// cache, then adding the System program to the processor's builtins.
pub(crate) fn create_transaction_batch_processor<CB: TransactionProcessingCallback>(
    callbacks: &CB,
    feature_set: &FeatureSet,
    compute_budget: &ComputeBudget,
    fork_graph: Arc<RwLock<PayTubeForkGraph>>,
) -> TransactionBatchProcessor<PayTubeForkGraph> {
    // @todo here we are using custom transaction batch processor, the example in the integration tests uses the TransactionBatchProcessor from the solana_svm crate. is it so?
    // I don't think so, that it is the issue with cache or TransactionBatchProcessor, But rather this is the issue with effective_slot, deployment_slot, root_slot(current slot) of the transaction context.
    // @todo transaction batch processor, is important for `load_and_execute_sanitized_transactions`.
    //
    let processor = TransactionBatchProcessor::<PayTubeForkGraph>::new(10, 2, HashSet::new());
    {
        let mut cache = processor.program_cache.write().unwrap();

        // Initialize the mocked fork graph.
        // let fork_graph = Arc::new(RwLock::new(PayTubeForkGraph {}));
        cache.fork_graph = Some(Arc::downgrade(&fork_graph));

        // Initialize a proper cache environment.
        // (Use Loader v4 program to initialize runtime v2 if desired)
        cache.environments.program_runtime_v1 = Arc::new(
            create_program_runtime_environment_v1(feature_set, compute_budget, false, false)
                .unwrap(),
        );

        // @todo all the programs that are going to be executed should be cached.
        // // Add the SPL Token program to the cache.
        // if let Some(program_account) = callbacks.get_account_shared_data(&spl_token::id()) {
        //     let elf_bytes = program_account.data();
        //     // println!("elf_bytes: {:?}", elf_bytes);
        //     let program_runtime_environment = cache.environments.program_runtime_v1.clone();
        //     cache.assign_program(
        //         spl_token::id(),
        //         Arc::new(
        //             ProgramCacheEntry::new(
        //                 &solana_sdk::bpf_loader::id(),
        //                 program_runtime_environment,
        //                 0,
        //                 0,
        //                 elf_bytes,
        //                 elf_bytes.len(),
        //                 &mut LoadProgramMetrics::default(),
        //             )
        //             .unwrap(),
        //         ),
        //     );
        // }
    }

    // Add the system program builtin.
    processor.add_builtin(
        callbacks,
        solana_system_program::id(),
        "system_program",
        ProgramCacheEntry::new_builtin(
            0,
            b"system_program".len(),
            system_processor::Entrypoint::vm,
        ),
    );

    // Add the BPF Loader v2 builtin, for the SPL Token program.
    processor.add_builtin(
        callbacks,
        solana_sdk::bpf_loader::id(),
        "solana_bpf_loader_program",
        ProgramCacheEntry::new_builtin(
            0,
            b"solana_bpf_loader_program".len(),
            solana_bpf_loader_program::Entrypoint::vm,
        ),
    );

    processor.add_builtin(
        callbacks,
        bpf_loader_upgradeable::id(),
        "solana_bpf_loader_upgradeable_program",
        ProgramCacheEntry::new_builtin(
            0,
            b"solana_bpf_loader_upgradeable_program".len(),
            solana_bpf_loader_program::Entrypoint::vm,
        ),
    );
    processor
}

/// This function is also a mock. In the Agave validator, the bank pre-checks
/// transactions before providing them to the SVM API. We mock this step in
/// PayTube, since we don't need to perform such pre-checks.
pub(crate) fn get_transaction_check_results(
    len: usize,
    lamports_per_signature: u64,
) -> Vec<transaction::Result<CheckedTransactionDetails>> {
    vec![
        transaction::Result::Ok(CheckedTransactionDetails {
            nonce: None,
            lamports_per_signature,
        });
        len
    ]
}

// @todo what else do we need to figrue?
// database key value marshalling and unmarshalling.
// system instruction marshalling and unmarshalling over rpc.
// new methods for deploying programs.
// post execution state updation methods. --> @todo look into this now. --> svm crate has collect_accounts_to_store method. this method gives the accounts changed for the given batch of transactions.
// charge fees for execution.
// signature verification is disabled. not sure how to enable it. --> if we want, we can verify transactions, in the usual signature verification way before execution and adding into mempool.
