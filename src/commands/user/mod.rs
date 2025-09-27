pub mod favorite;
pub mod numberone;
pub mod progress;
pub mod readinglist;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn user_commands() -> CommandVec {
    vec![
        favorite::favorite(),
        progress::progress(),
        readinglist::readinglist(),
        numberone::numberone(),
    ]
}
