mod client;
mod server;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about)]
pub struct Cli {
    #[arg(long, global = true)]
    pub upstream: Option<String>,
    #[arg(long, global = true)]
    pub project: Option<String>,
    #[arg(long, global = true)]
    pub json: bool,
    #[arg(long, global = true)]
    pub allow_http: bool,
    #[command(subcommand)]
    pub command: Command,
}
#[derive(Subcommand)]
pub enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long)]
        public_url: String,
    },
    AdminKey {
        #[arg(long)]
        data_dir: PathBuf,
    },
    New(AuthArgs),
    Upload {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    Sync {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
    },
    Delete {
        files: Vec<String>,
    },
    Config(AuthArgs),
    Keys {
        #[command(subcommand)]
        command: KeyCommand,
    },
}
#[derive(Args)]
pub struct AuthArgs {
    #[arg(long, num_args=0..=1, default_missing_value="", conflicts_with="no_basic_auth")]
    pub basic_auth: Option<String>,
    #[arg(long, conflicts_with = "no_basic_auth")]
    pub password_stdin: bool,
    #[arg(long)]
    pub no_basic_auth: bool,
}
#[derive(Subcommand)]
pub enum KeyCommand {
    Create {
        #[arg(long, default_value = "upload,sync")]
        scope: String,
        #[arg(long)]
        expires_in: Option<u64>,
    },
    List,
    Revoke {
        key_id: String,
    },
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Serve {
            listen,
            data_dir,
            public_url,
        } => server::serve(listen, data_dir, public_url).await,
        Command::AdminKey { data_dir } => server::admin_key(data_dir, cli.json),
        _ => client::run(&cli).await,
    };
    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

pub fn hash(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}
pub fn id(bytes: usize) -> String {
    use rand::RngCore;
    let mut data = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut data);
    hex::encode(data)
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
pub fn valid_path(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 1024
        && !p.contains(['\\', '\0', ':'])
        && p.split('/').all(|x| {
            !x.is_empty()
                && x != "."
                && x != ".."
                && !x.starts_with('.')
                && !matches!(x, "node_modules" | "target")
                && !x.ends_with(".pem")
                && !x.ends_with(".key")
        })
}
