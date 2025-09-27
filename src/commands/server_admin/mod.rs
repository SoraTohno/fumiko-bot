pub mod adminprogress;
pub mod adminqueue;
pub mod clubreadadd;
pub mod clubreadremove;
pub mod config;
pub mod finishbook;
pub mod mature;
pub mod select;
pub mod setup;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn server_admin_commands() -> CommandVec {
    vec![
        config::config(),
        finishbook::finishbook(),
        select::select(),
        setup::setup(),
        clubreadremove::clubreadremove(),
        adminqueue::adminqueue(),
        clubreadadd::clubreadadd(),
        adminprogress::adminprogress(),
    ]
}
