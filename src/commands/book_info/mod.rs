// once implementation of next/queue/select has been determined (and its vocab) include random book selector from db?
// Declare the command files in this sub-module
pub mod botinfo;
pub mod info;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn book_info_commands() -> CommandVec {
    vec![info::book(), info::author(), botinfo::botinfo()]
}
