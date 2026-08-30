use tmux_atelier::app;

fn main() {
    if let Err(error) = app::run() {
        eprintln!("tmux-atelier: {error}");
        std::process::exit(1);
    }
}
