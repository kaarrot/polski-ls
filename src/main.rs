mod backend;
mod dictionary;
mod pos_conv;

use backend::Backend;
use clap::Parser;
use tower_lsp_server::{LspService, Server};

#[derive(Debug, Parser)]
#[command(version, about = "Polish language LSP server with completion support")]
struct Args {
    /// Listen on standard input/output rather than TCP.
    #[arg(short, long, default_value_t = false)]
    stdio: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let (service, socket) = LspService::new(Backend::new);

    if args.stdio {
        let stdin = tokio::io::stdin();
        let stdout = tokio::io::stdout();
        Server::new(stdin, stdout, socket).serve(service).await;
    } else {
        eprintln!("TCP mode not implemented. Use --stdio");
    }
}
