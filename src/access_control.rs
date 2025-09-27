use crate::types::{Context, Error};
use poise::CreateReply;

const GUILD_REQUIRED_MESSAGE: &str =
    "This command must be used in a server where the bot is installed.";

pub async fn command_gate(ctx: Context<'_>) -> Result<bool, Error> {
    let Some(guild_id) = ctx.guild_id() else {
        ctx.send(CreateReply::default().content(GUILD_REQUIRED_MESSAGE))
            .await?;
        return Ok(false);
    };

    let is_known_guild = {
        let guilds = ctx.data().guild_cache.read().await;
        guilds.contains(&guild_id)
    };

    if !is_known_guild {
        ctx.send(
            CreateReply::default()
                .content(GUILD_REQUIRED_MESSAGE)
                .ephemeral(true),
        )
        .await?;
        return Ok(false);
    }

    Ok(true)
}
