pub mod book_info;
pub mod explore;
pub mod helper;
pub mod server_admin;
pub mod server_user;
pub mod user;

use crate::{types::Data, types::Error};

type CommandVec = Vec<poise::Command<Data, Error>>;

pub fn all_commands() -> CommandVec {
    let mut commands = Vec::new();

    commands.extend(book_info::book_info_commands());
    commands.extend(explore::explore_commands());
    commands.extend(helper::helper_commands());
    commands.extend(server_user::server_user_commands());
    commands.extend(server_admin::server_admin_commands());
    commands.extend(user::user_commands());

    commands
}
