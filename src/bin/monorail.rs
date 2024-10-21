use monorail::cli;
use monorail::tracing;

#[tokio::main]
async fn main() {
    let app = cli::get_app();
    let matches = app.get_matches();
    let output_format = matches.get_one::<String>(cli::ARG_OUTPUT_FORMAT).unwrap();
    let verbosity = matches.get_one::<u8>(cli::ARG_VERBOSE).unwrap_or(&0);

    tracing::setup(&output_format, *verbosity).unwrap();

    match cli::handle(&matches, output_format).await {
        Ok(code) => {
            std::process::exit(code);
        }
        Err(e) => {
            cli::write_result::<()>(&Err(e), output_format).expect("Failed to write fatal result");
            std::process::exit(cli::HANDLE_FATAL);
        }
    }
}
