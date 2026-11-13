//! A distribution's `serve` (#1156): register the modules, then run the
//! library's server. With `examples/hello_module.rs`'s `::HELLO`:
//!
//!     cargo run --features serve --example hello_serve -- --dashboards dir/
#[path = "hello_module.rs"]
mod hello_module;

fn main() {
    if let Err(e) = anofox_visualization::module::register(&hello_module::HELLO) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    anofox_visualization::cli::serve::main()
}
