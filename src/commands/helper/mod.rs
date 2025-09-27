pub mod deletedata;
pub mod help;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn helper_commands() -> CommandVec {
    vec![deletedata::deletedata(), help::help()]
}
