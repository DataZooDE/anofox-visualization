//! A distribution's `dashboard` (#1156): register the modules, then run the
//! library's command. With `examples/hello_module.rs`'s `::HELLO`:
//!
//!     echo "SELECT 'world'::HELLO;" > hello.sql
//!     cargo run --example hello_dashboard -- hello.sql hello.html
#[path = "hello_module.rs"]
mod hello_module;

fn main() {
    if let Err(e) = anofox_visualization::module::register(&hello_module::HELLO) {
        eprintln!("{e}");
        std::process::exit(2);
    }
    anofox_visualization::cli::dashboard::main()
}
