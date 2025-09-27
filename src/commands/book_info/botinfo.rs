use crate::util::embed_author_with_icon;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter};

#[poise::command(slash_command, user_cooldown = 10)]
pub async fn botinfo(ctx: Context<'_>) -> Result<(), Error> {
    let bot_face = ctx.serenity_context().http.get_current_user().await?.face();

    ctx.send(poise::CreateReply::default().embed(
        CreateEmbed::new()
            .author(embed_author_with_icon(
                "Fumiko Book Club Bot Info",
                Some(bot_face),
            ))
            .description(
                "A Discord bot for managing book clubs, reading lists, and book discussions.\n\n\
                 **Features:**\n\
                 • Book queue management\n\
                 • Reading progress tracking\n\
                 • Track reading history and rankings\n\
                 • Personal reading lists and favorites\n\n\
                 Book data is provided by the Google Books API.\n\n\
                 If you encounter any issues, please report them us via [fumiko.dev/contact](https://fumiko.dev/contact)."
            )
            .field("Commands", "Please go to [fumiko.dev/commands](https://fumiko.dev/commands) or use `/help` to see all available commands", false)
            .field(
                "Server Setup",
                "Administrators can configure per-server bot settings with `/config`. For information on available config commands, you can use `/setup`.",
                false,
            )
            .field("Data Source", "[Google Books API](https://developers.google.com/books)", false)
            .footer(CreateEmbedFooter::new(
                "Fumiko Book Club Bot • fumiko.dev"
            ))
            .color(0xB76E79) // pastel pink
    )).await?;

    Ok(())
}
