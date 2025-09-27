use crate::util::get_guild_name;
use crate::*;
use crate::{types::Context, types::Error};
use poise::serenity_prelude::{CreateEmbed, CreateEmbedFooter, User};

#[poise::command(
    slash_command,
    subcommands("remove", "ban", "unban"),
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized("en-US", "Administrative tools for managing /progress updates"),
    user_cooldown = 5
)]
pub async fn adminprogress(_ctx: Context<'_>) -> Result<(), Error> {
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Remove a member's current /progress update (requires Manage Server)",
    ),
    user_cooldown = 5
)]
async fn remove(
    ctx: Context<'_>,
    #[description = "User to clear progress for"] user: User,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_user_exists(pool, &user).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let current_book = sqlx::query!(
        r#"
        SELECT
            volume_id
        FROM server_current_book
        WHERE server_id = $1
        "#,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let Some(book) = current_book else {
        let embed = CreateEmbed::default()
            .title("No Current Book")
            .description(
                "There's no current book being read in this server, so there isn't any progress to remove.",
            )
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let volume_id = book.volume_id;
    let book_title = match google_books.get_volume(&volume_id).await {
        Ok(volume) => volume.get_title(),
        Err(_) => format!("Book ({})", volume_id),
    };

    let removed = sqlx::query!(
        "DELETE FROM user_reading_progress WHERE user_id = $1 AND server_id = $2 AND volume_id = $3 RETURNING progress_text",
        user.id.get() as i64,
        guild_id.get() as i64,
        volume_id
    )
    .fetch_optional(pool)
    .await?;

    if removed.is_some() {
        let embed = CreateEmbed::default()
            .title("Progress Removed")
            .description(format!(
                "{}'s progress for '{}' has been cleared.",
                user.name, book_title
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    } else {
        let embed = CreateEmbed::default()
            .title("No Progress Found")
            .description(format!(
                "{} doesn't have any tracked progress for '{}' right now.",
                user.name, book_title
            ))
            .color(0xB76E79)
            .footer(CreateEmbedFooter::new("Powered by Google Books API"));
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
    }

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Ban a member from using /progress and clear their current entry (requires Manage Server)",
    ),
    user_cooldown = 5
)]
async fn ban(
    ctx: Context<'_>,
    #[description = "User to ban from /progress"] user: User,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let google_books = &ctx.data().google_books;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_user_exists(pool, &user).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let cleared_progress = sqlx::query!(
        "DELETE FROM user_reading_progress WHERE user_id = $1 AND server_id = $2 RETURNING volume_id",
        user.id.get() as i64,
        guild_id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    sqlx::query!(
        "INSERT INTO progress_command_bans (server_id, user_id, banned_by, banned_at) VALUES ($1, $2, $3, CURRENT_TIMESTAMP) ON CONFLICT (server_id, user_id) DO UPDATE SET banned_by = $3, banned_at = CURRENT_TIMESTAMP",
        guild_id.get() as i64,
        user.id.get() as i64,
        ctx.author().id.get() as i64
    )
    .execute(pool)
    .await?;

    let mut description = format!(
        "{} has been banned from using /progress commands in this server.",
        user.name
    );

    if let Some(record) = cleared_progress {
        let volume_id = record.volume_id;
        let book_title = match google_books.get_volume(&volume_id).await {
            Ok(volume) => volume.get_title(),
            Err(_) => format!("Book ({})", volume_id),
        };
        description.push_str(&format!(
            " Their saved progress for '{}' was cleared.",
            book_title
        ));
    } else {
        description.push_str(" They didn't have any saved progress to clear.");
    }

    let embed = CreateEmbed::default()
        .title("Progress Command Ban Applied")
        .description(description)
        .color(0xB76E79)
        .footer(CreateEmbedFooter::new("Powered by Google Books API"));

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    required_permissions = "MANAGE_GUILD",
    description_localized(
        "en-US",
        "Unban a member from using /progress commands (requires Manage Server)",
    ),
    user_cooldown = 5
)]
async fn unban(
    ctx: Context<'_>,
    #[description = "User to unban from /progress"] user: User,
) -> Result<(), Error> {
    ctx.defer().await?;

    let pool = &ctx.data().database;
    let Some(guild_id) = ctx.guild_id() else {
        let embed = CreateEmbed::default()
            .title("❌ Error")
            .description("This command must be used in a server.")
            .color(0xB76E79);
        ctx.send(poise::CreateReply::default().embed(embed)).await?;
        return Ok(());
    };

    let guild_name = get_guild_name(&ctx).await;

    ensure_user_exists(pool, ctx.author()).await?;
    ensure_user_exists(pool, &user).await?;
    ensure_server_exists(pool, guild_id, &guild_name).await?;

    let unbanned = sqlx::query!(
        "DELETE FROM progress_command_bans WHERE server_id = $1 AND user_id = $2 RETURNING banned_at",
        guild_id.get() as i64,
        user.id.get() as i64
    )
    .fetch_optional(pool)
    .await?;

    let (title, description) = if unbanned.is_some() {
        (
            "Progress Command Ban Removed",
            format!(
                "{} can now use /progress commands again in this server.",
                user.name
            ),
        )
    } else {
        (
            "No Ban Found",
            format!(
                "{} wasn't banned from using /progress commands in this server.",
                user.name
            ),
        )
    };

    let embed = CreateEmbed::default()
        .title(title)
        .description(description)
        .color(0xB76E79);

    ctx.send(poise::CreateReply::default().embed(embed)).await?;

    Ok(())
}
