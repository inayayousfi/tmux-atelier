pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
struct MessageError(String);

impl std::fmt::Display for MessageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

pub fn err(message: impl Into<String>) -> Error {
    Box::new(MessageError(message.into()))
}

pub mod app;
pub(crate) mod command;
pub mod config;
pub mod interaction;
pub mod process;
pub mod process_state;
pub mod snapshot;
pub mod wizard;
pub mod workspace;
