use std::env;

use crate::command::App;
use crate::{err, Result};

pub fn run() -> Result<()> {
    let mut arguments = env::args().skip(1);
    let Some(command) = arguments.next() else {
        App::usage();
        return Err(err("command is required"));
    };
    App::from_env()?.dispatch(&command, &arguments.collect::<Vec<_>>())
}
