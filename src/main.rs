use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "synoctl", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Placeholder until the command tree lands
    Info,
}

fn main() {
    let _ = Cli::parse();
}
