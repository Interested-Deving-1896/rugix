pub mod boot;
pub mod cli;
pub mod config;
pub mod http_source;
pub mod init;
pub mod overlay;
pub mod slot_db;
pub mod state;
pub mod system;
pub mod system_state;
pub mod utils;

pub fn main() {
    if rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .is_err()
    {
        // This should never happen as there is no pre-installed provider at this
        // point, so the installation should always succeed.
        eprintln!("unable to install default crypto provider, continuing anyway");
    }
    let result = rugix_tasks::run(|| {
        if utils::is_init_process() {
            init::main()
        } else {
            cli::main()
        }
    });
    if let Err(report) = result {
        eprintln!("{report:?}");
        std::process::exit(1);
    }
}
