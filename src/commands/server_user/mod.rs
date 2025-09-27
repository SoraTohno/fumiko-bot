pub mod clubrating;
pub mod clubread;
pub mod current;
pub mod queue;
pub mod stats;
pub mod userrating;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn server_user_commands() -> CommandVec {
    vec![
        clubrating::clubrating(),
        clubread::clubread(),
        current::current(),
        queue::queue(),
        stats::stats(),
        userrating::userrating(),
    ]
}
