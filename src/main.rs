mod auth;
mod client;
mod expiry;
mod server;
mod update;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    version,
    about = "Simple Static Server: publish static files from any machine",
    subcommand_required = false
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        env = "SSS_UPSTREAM",
        default_value = "http://localhost:8080"
    )]
    pub upstream: String,
    #[arg(long, global = true)]
    pub project: Option<String>,
    #[arg(
        long = "basic_auth",
        global = true,
        env = "SSS_BASIC_AUTH",
        hide_env_values = true
    )]
    pub basic_auth: Option<String>,
    #[arg(long)]
    pub skill: bool,
    #[command(subcommand)]
    pub command: Option<Command>,
}
#[derive(Subcommand)]
pub enum Command {
    /// Run the HTTP server.
    Serve {
        #[arg(long, env = "SSS_LISTEN", default_value = "127.0.0.1")]
        listen: std::net::IpAddr,
        #[arg(long, env = "SSS_PORT", default_value_t = 8080)]
        port: u16,
        #[arg(long, env = "SSS_DATA_DIR")]
        data_dir: Option<PathBuf>,
        #[arg(long, env = "SSS_PUBLIC_URL")]
        public_url: Option<String>,
        /// Default lifetime for new projects (e.g. 7d, 12h, or none).
        #[arg(long, env = "SSS_DEFAULT_EXPIRES_IN", default_value = "none")]
        default_expires_in: String,
    },
    /// Create a project. Does not change the working directory or save credentials.
    New(ProjectArgs),
    /// List projects (shared authentication).
    List,
    /// Show project URLs, storage usage, access rules, and expiration.
    Info,
    /// Diagnose server connectivity and authentication.
    Doctor,
    /// Compare local files with the published snapshot without changing it.
    Diff { dir: PathBuf },
    /// Add or replace selected files.
    Upload {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Mirror a directory, including deletions.
    Sync {
        dir: PathBuf,
        #[arg(long)]
        dry_run: bool,
        /// Publish after changes settle; Ctrl-C stops watching.
        #[arg(long, conflicts_with = "dry_run")]
        watch: bool,
    },
    /// Delete selected files, or the entire project when no files are specified.
    Delete { files: Vec<String> },
    /// Change project name, authentication, or expiration (shared authentication).
    Config(ProjectArgs),
    /// List retained versions.
    Versions,
    /// Select a retained version as current.
    Rollback { version: u64 },
    /// Delete a retained version other than the current version.
    DeleteVersion { version: u64 },
    /// Install the latest verified release and refresh registered embedded skills.
    Update {
        #[arg(long)]
        check: bool,
    },
}
#[derive(Args)]
pub struct ProjectArgs {
    #[arg(long)]
    pub name: Option<String>,
    /// Lifetime from now (e.g. 7d, 12h, or none to disable expiry).
    #[arg(long)]
    pub expires_in: Option<String>,
    #[arg(long)]
    pub config: Option<PathBuf>,
    #[arg(long = "basic_auth_view")]
    pub view: Option<String>,
    #[arg(long = "basic_auth_write")]
    pub write: Option<String>,
}
#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if cli.skill {
        print!("{}", include_str!("../skill.md"));
        return;
    }
    let result = match &cli.command {
        Some(Command::Serve {
            listen,
            port,
            data_dir,
            public_url,
            default_expires_in,
        }) => match data_dir.clone().map(Ok).unwrap_or_else(default_data_dir) {
            Ok(root) => {
                let url = public_url
                    .clone()
                    .unwrap_or_else(|| format!("http://localhost:{port}"));
                server::serve(
                    std::net::SocketAddr::new(*listen, *port),
                    &root,
                    &url,
                    cli.basic_auth.as_deref(),
                    default_expires_in,
                )
                .await
            }
            Err(e) => Err(e),
        },
        Some(Command::Update { check }) => update::run(*check).await,
        Some(_) => client::run(&cli).await,
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().map_err(Into::into)
        }
    };
    if let Err(e) = result {
        eprintln!("{}", serde_json::json!({"error":format!("{e:#}")}));
        std::process::exit(1);
    }
}
fn default_data_dir() -> anyhow::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("HOME")
            .ok_or_else(|| anyhow::anyhow!("HOME is unset; provide --data-dir"))?,
    )
    .join(".sss"))
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
        && p.split('/').next() != Some("versions")
}
