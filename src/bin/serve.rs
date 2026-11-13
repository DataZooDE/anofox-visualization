//! `serve` — the builder and dashboard server; the program lives in the
//! library ([`anofox_visualization::cli::serve`]) so a distribution crate can
//! run it with its own modules registered (#1156).
fn main() {
    anofox_visualization::cli::serve::main()
}
