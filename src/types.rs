use crate::google_books_cache::CachedGoogleBooksClient;
use poise::serenity_prelude as serenity;
use sqlx::postgres::PgPool;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

pub struct Data {
    pub database: PgPool,
    // pub google_books: GoogleBooksClient,
    pub google_books: CachedGoogleBooksClient,
    pub guild_cache: Arc<RwLock<HashSet<serenity::GuildId>>>,
}

#[derive(poise::ChoiceParameter, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryMode {
    // #[name = "auto"] Auto,
    #[name = "title"]
    Title,
    #[name = "isbn"]
    Isbn,
}
