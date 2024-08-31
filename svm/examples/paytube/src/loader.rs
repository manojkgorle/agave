//! PayTube's "account loader" component, which provides the SVM API with the
//! ability to load accounts for PayTube channels.
//!
//! The account loader is a simple example of an RPC client that can first load
//! an account from the base chain, then cache it locally within the protocol
//! for the duration of the channel.

use {
    pickledb::{PickleDb, PickleDbDumpPolicy, SerializationMethod},
    serde::Serialize,
    solana_client::rpc_client::RpcClient,
    solana_sdk::bpf_loader_upgradeable::UpgradeableLoaderState,
    solana_sdk::{
        account::{AccountSharedData, ReadableAccount, WritableAccount},
        bpf_loader_upgradeable,
        pubkey::Pubkey,
        slot_history::Slot,
    },
    solana_svm::transaction_processing_callback::TransactionProcessingCallback,
    std::{
        collections::HashMap,
        env,
        fs::{self, File},
        io::Read,
        sync::RwLock,
    },
};

/// An account loading mechanism to hoist accounts from the base chain up to
/// an active PayTube channel.
///
/// Employs a simple cache mechanism to ensure accounts are only loaded once.
pub struct PayTubeAccountLoader<'a> {
    pub cache: RwLock<HashMap<Pubkey, AccountSharedData>>,
    rpc_client: &'a RpcClient,
}

impl<'a> PayTubeAccountLoader<'a> {
    pub fn new(rpc_client: &'a RpcClient) -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            rpc_client,
        }
    }
}

/// Implementation of the SVM API's `TransactionProcessingCallback` interface.
///
/// The SVM API requires this plugin be provided to provide the SVM with the
/// ability to load accounts.
///
/// In the Agave validator, this implementation is Bank, powered by AccountsDB.
impl TransactionProcessingCallback for PayTubeAccountLoader<'_> {
    // @todo this is a very simple way, we did not initiate a banck directly(but did indirectly). So, we are caching, the accounts. If the account is not in cache, then we are fetching the account with a rpc call.
    // Haha, we can think in this way too, as this is a side channel, for payments, instead of keeping track of records directly, we take them from original solana, if ever accessed for the first time.
    // this is a very naive example. There is no locking of funds on some solana contract, when using this side channel.
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        // println!(
        //     "account shared data: {:?}, \npubkey query: {:?}",
        //     self.cache.read(),
        //     pubkey
        // );
        println!("pubkey: {:?}", pubkey);
        if let Some(account) = self.cache.read().unwrap().get(pubkey) {
            return Some(account.clone());
        }

        let account: AccountSharedData = self.rpc_client.get_account(pubkey).ok()?.into();
        self.cache.write().unwrap().insert(*pubkey, account.clone());

        Some(account)
    }

    fn account_matches_owners(&self, account: &Pubkey, owners: &[Pubkey]) -> Option<usize> {
        self.get_account_shared_data(account)
            .and_then(|account| owners.iter().position(|key| account.owner().eq(key)))
    }
}

pub struct PayTubeAccountLoaderWithLocalDB {
    db: PickleDb,
}

impl PayTubeAccountLoaderWithLocalDB {
    pub fn new() -> Self {
        let db = PickleDb::new(
            "paytube_accounts.db",
            PickleDbDumpPolicy::AutoDump,
            SerializationMethod::Json,
        );
        Self { db }
    }

    // @todo just write lamports. Here we are not offering any smart contract functionalities.
    pub fn write_to_db(&mut self, pubkey: &Pubkey, account: &AccountSharedData) {
        self.db
            .set(&pubkey.to_string(), &account.lamports())
            .unwrap();
    }

    pub fn read_from_db(&self, pubkey: &Pubkey) -> Option<u64> {
        self.db.get::<u64>(&pubkey.to_string())
    }

    pub fn write_genesis(&mut self, pubkeys: &[Pubkey], accounts: &[AccountSharedData]) {
        for (pubkey, account) in pubkeys.iter().zip(accounts.iter()) {
            self.write_to_db(pubkey, account);
        }
    }
}

impl TransactionProcessingCallback for PayTubeAccountLoaderWithLocalDB {
    fn get_account_shared_data(&self, pubkey: &Pubkey) -> Option<AccountSharedData> {
        let account = self.db.get::<AccountSharedData>(&pubkey.to_string());
        account
    }

    fn account_matches_owners(&self, account: &Pubkey, owners: &[Pubkey]) -> Option<usize> {
        self.get_account_shared_data(account)
            .and_then(|account| owners.iter().position(|key| account.owner().eq(key)))
    }
}

pub fn deploy_program(
    name: String,
    deployment_slot: Slot,
    mock_bank: &PayTubeAccountLoader,
) -> Pubkey {
    // @todo lmao this pattern is like uups, upgradable pattern.
    let program_account = Pubkey::new_unique();
    let program_data_account = Pubkey::new_unique();
    let state = UpgradeableLoaderState::Program {
        programdata_address: program_data_account,
    };

    // The program account must have funds and hold the executable binary
    let mut account_data = AccountSharedData::default();
    account_data.set_data_from_slice(&bincode::serialize(&state).unwrap());
    account_data.set_lamports(25);
    account_data.set_owner(bpf_loader_upgradeable::id()); // @todo bpf_loader_upgradeable is interesting. bpf_loader_upgradeable has authority to deploy, upgrade and execute programs.
    mock_bank
        .cache
        .write()
        .unwrap()
        .insert(program_account, account_data);

    let mut account_data = AccountSharedData::default();
    let state = UpgradeableLoaderState::ProgramData {
        slot: deployment_slot, // @todo program activation slot.
        upgrade_authority_address: None,
    };
    let mut header = bincode::serialize(&state).unwrap();
    let mut complement = vec![
        0;
        std::cmp::max(
            0,
            UpgradeableLoaderState::size_of_programdata_metadata().saturating_sub(header.len())
        )
    ];
    let mut buffer = load_program(name);
    header.append(&mut complement);
    header.append(&mut buffer);
    account_data.set_data_from_slice(&header);
    mock_bank
        .cache
        .write()
        .unwrap()
        .insert(program_data_account, account_data);
    println!("program_account: {:?}", program_account);
    program_account
}

fn load_program(name: String) -> Vec<u8> {
    // Loading the program file
    let mut dir = env::current_dir().unwrap();
    dir.push("tests");
    dir.push("example-programs");
    dir.push(name.as_str());
    let name = name.replace('-', "_");
    dir.push(name + "_program.so");
    let mut file = File::open(dir.clone()).expect("file not found");
    let metadata = fs::metadata(dir).expect("Unable to read metadata");
    let mut buffer = vec![0; metadata.len() as usize];
    file.read_exact(&mut buffer).expect("Buffer overflow");
    buffer
}
