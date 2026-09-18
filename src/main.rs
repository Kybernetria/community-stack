use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use community_stack::{
    adapters::{
        hashing::Blake3ContentHasher,
        local_api,
        loro::LoroDocumentEngine,
        p2panda::P2pandaSecureLog,
        sqlite::{self, StoreHandle},
    },
    application::CommunityCore,
    config, recovery,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "community-stack",
    version,
    about = "Device-local replicated community backend"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create the database and device-local cryptographic keys.
    Init {
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
    },
    /// Create or rotate an application/transport capability token.
    Register {
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
        #[arg(long)]
        id: String,
        #[arg(long, value_enum)]
        role: Role,
        /// Write the capability to a new private file instead of stdout.
        #[arg(long)]
        token_file: Option<PathBuf>,
    },
    /// Create a complete device backup; destination must not exist.
    Backup {
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
        #[arg(long)]
        destination: PathBuf,
    },
    /// Verify a backup without modifying it.
    VerifyBackup {
        #[arg(long)]
        source: PathBuf,
    },
    /// Recover the same device into a new directory.
    Restore {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Run the stable local Unix-socket entrypoint.
    Serve {
        #[arg(long, default_value = "./data")]
        data_dir: PathBuf,
        #[arg(long)]
        socket: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Role {
    App,
    Admin,
    Transport,
}

impl Role {
    const fn database_name(self) -> &'static str {
        match self {
            Self::App => "APP",
            Self::Admin => "ADMIN",
            Self::Transport => "TRANSPORT",
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("community_stack=info")),
        )
        .with_target(false)
        .compact()
        .init();

    match Cli::parse().command {
        Command::Init { data_dir } => {
            let _ownership = config::lock_data_dir(&data_dir)?;
            config::initialize_data_dir(&data_dir)?;
            sqlite::initialize(&config::database_path(&data_dir))?;
            println!("initialized {}", data_dir.display());
            println!(
                "next: community-stack register --data-dir {} --id <app-id> --role app",
                data_dir.display()
            );
        }
        Command::Register {
            data_dir,
            id,
            role,
            token_file,
        } => {
            let _ownership = config::lock_data_dir(&data_dir)?;
            config::initialize_data_dir(&data_dir)?;
            let (token_hash, token) = config::generate_token()?;
            if let Some(path) = &token_file {
                config::write_token_file(path, &token)?;
            }
            if let Err(error) = sqlite::register_application(
                &config::database_path(&data_dir),
                &id,
                &token_hash,
                role.database_name(),
            ) {
                if let Some(path) = &token_file {
                    let _ = std::fs::remove_file(path);
                }
                return Err(error);
            }
            println!("principal={id}");
            println!("role={}", role.database_name());
            if let Some(path) = token_file {
                println!("token_file={}", path.display());
                eprintln!(
                    "Capability written to a new private token file; registering the same id again rotates it."
                );
            } else {
                println!("token={token}");
                eprintln!("Store this token securely; registering the same id again rotates it.");
            }
        }
        Command::Backup {
            data_dir,
            destination,
        } => {
            recovery::backup(&data_dir, &destination)?;
            println!("backup verified: {}", destination.display());
        }
        Command::VerifyBackup { source } => {
            recovery::verify(&source)?;
            println!("backup verified: {}", source.display());
        }
        Command::Restore { source, data_dir } => {
            recovery::restore(&source, &data_dir)?;
            println!("device restored: {}", data_dir.display());
            eprintln!(
                "This preserves device identity. Stop the original before serving the restored copy."
            );
        }
        Command::Serve { data_dir, socket } => {
            let _ownership = config::lock_data_dir(&data_dir)?;
            let (signing_key, master_key) = config::load_keys(&data_dir).with_context(|| {
                format!("load keys from {}; run init first", data_dir.display())
            })?;
            let repository = Arc::new(StoreHandle::start(&config::database_path(&data_dir))?);
            let mut peer_bytes = [0_u8; 8];
            getrandom::fill(&mut peer_bytes)
                .map_err(|error| anyhow::anyhow!("OS random source failed: {error}"))?;
            let writer_peer_id = u64::from_be_bytes(peer_bytes);
            let documents = Arc::new(LoroDocumentEngine);
            let secure_log = Arc::new(P2pandaSecureLog::new(signing_key, master_key));
            let content_hasher = Arc::new(Blake3ContentHasher);
            let core = CommunityCore::new(
                repository,
                documents,
                secure_log,
                content_hasher,
                writer_peer_id,
            );
            let socket = socket.unwrap_or_else(|| config::socket_path(&data_dir));
            info!(author = %core.call_public("health").await?["author_key"], "community core started");
            local_api::serve(&socket, core).await?;
        }
    }
    Ok(())
}
