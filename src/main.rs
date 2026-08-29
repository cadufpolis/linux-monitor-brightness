use openaction::OpenActionResult;

mod gfx;
mod monitors;
mod plugin;

#[tokio::main]
async fn main() -> OpenActionResult<()> {
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Debug,
        simplelog::Config::default(),
        simplelog::TerminalMode::Stdout,
        simplelog::ColorChoice::Never,
    )
    .unwrap();

    println!("Starting Monitor Brightness plugin...");

    plugin::init().await
}
