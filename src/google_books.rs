use crate::util::truncate_on_char_boundary;
use anyhow::{Context, Result, anyhow};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};

const GOOGLE_BOOKS_API_BASE: &str = "https://www.googleapis.com/books/v1";

#[derive(Debug, Clone)]
pub struct GoogleBooksClient {
    client: Client,
    api_key: Option<String>,
    base_url: Url,
}

impl GoogleBooksClient {
    pub fn new(api_key: Option<String>) -> Self {
        let base_url = Self::normalize_base_url(
            Url::parse(GOOGLE_BOOKS_API_BASE).expect("GOOGLE_BOOKS_API_BASE should be a valid URL"),
        );
        Self {
            client: Client::new(),
            api_key,
            base_url,
        }
    }

    #[cfg(test)]
    pub(crate) fn new_with_base_url(api_key: Option<String>, base_url: &str) -> Self {
        let base_url = Self::normalize_base_url(
            Url::parse(base_url).expect("invalid Google Books base URL for tests"),
        );
        Self {
            client: Client::new(),
            api_key,
            base_url,
        }
    }

    fn normalize_base_url(mut base_url: Url) -> Url {
        if !base_url.path().ends_with('/') {
            let mut path = base_url.path().to_owned();
            path.push('/');
            base_url.set_path(&path);
        }
        base_url
    }

    // Search by title (optionally constrain by author). Returns up to `max_results` (capped at 40).
    pub async fn search_books(
        &self,
        title: &str,
        author: Option<&str>,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let mut q = format!("intitle:{}", title);
        if let Some(a) = author {
            q.push(' ');
            q.push_str(&format!("inauthor:{a}"));
        }

        let mut url = self.base_url.join("volumes")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("q", &q);
            qp.append_pair("maxResults", &max_results.unwrap_or(10).min(40).to_string());
            if let Some(key) = &self.api_key {
                qp.append_pair("key", key);
            }
        }

        let body = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()
            .context("Google Books HTTP error")?
            .text()
            .await?;

        let parsed: SearchResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow!(
                "Failed to decode Google Books JSON: {e}; body: {}",
                truncate(&body, 900)
            )
        })?;

        Ok(parsed.items.unwrap_or_default())
    }

    // Search a single volume by ISBN (10 or 13).
    pub async fn search_by_isbn(&self, isbn: &str) -> Result<Option<Volume>> {
        let mut url = self.base_url.join("volumes")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("q", &format!("isbn:{isbn}"));
            if let Some(key) = &self.api_key {
                qp.append_pair("key", key);
            }
        }

        let body = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let mut items = serde_json::from_str::<SearchResponse>(&body)
            .map_err(|e| {
                anyhow!(
                    "Failed to decode Google Books JSON: {e}; body: {}",
                    truncate(&body, 900)
                )
            })?
            .items
            .unwrap_or_default();

        Ok(items.drain(..).next())
    }

    // Search by author only (used for /author or fallbacks).
    pub async fn search_by_author(
        &self,
        author: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let mut url = self.base_url.join("volumes")?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("q", &format!("inauthor:{author}"));
            qp.append_pair("maxResults", &max_results.unwrap_or(10).min(40).to_string());
            if let Some(key) = &self.api_key {
                qp.append_pair("key", key);
            }
        }

        let body = self
            .client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let parsed: SearchResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow!(
                "Failed to decode Google Books JSON: {e}; body: {}",
                truncate(&body, 900)
            )
        })?;

        Ok(parsed.items.unwrap_or_default())
    }

    // Fetch a specific volume by its Google Books volume ID.
    pub async fn get_volume(&self, volume_id: &str) -> Result<Volume> {
        // let mut url = self.base_url.join("volumes/")?;
        let mut url = self.base_url.join("volumes")?;
        {
            // push the id as a path segment to ensure correct encoding
            url.path_segments_mut()
                .map_err(|_| anyhow!("Cannot be base for path segments"))?
                .push(volume_id);
            if let Some(key) = &self.api_key {
                url.query_pairs_mut().append_pair("key", key);
            }
        }

        let requested_path = url.path().to_owned();
        let response = self.client.get(url).send().await.with_context(|| {
            format!("Failed to send Google Books request for volume {volume_id}")
        })?;

        let status = response.status();
        let response_host = response.url().host_str().unwrap_or("").to_owned();
        let body = response.text().await.with_context(|| {
            format!("Failed to read Google Books response body for volume {volume_id}")
        })?;

        if !status.is_success() {
            let reason = status.canonical_reason().unwrap_or("Unknown");
            let truncated_body = truncate(&body, 900);
            eprintln!(
                "Google Books API returned {} {} for volume {} ({}{}): {}",
                status.as_u16(),
                reason,
                volume_id,
                response_host,
                requested_path,
                truncated_body
            );
            return Err(anyhow!(
                "Google Books API error for volume {} (status {} {}): {}",
                volume_id,
                status.as_u16(),
                reason,
                truncated_body
            ));
        }

        let volume: Volume = serde_json::from_str(&body).map_err(|e| {
            let truncated_body = truncate(&body, 900);
            eprintln!(
                "Failed to decode Google Books response for volume {} ({}{}): {e}; body: {}",
                volume_id, response_host, requested_path, truncated_body
            );
            anyhow!(
                "Failed to decode Google Books Volume JSON: {e}; body: {}",
                truncated_body
            )
        })?;

        Ok(volume)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct SearchResponse {
    pub items: Option<Vec<Volume>>,
    #[serde(default, rename = "totalItems")]
    pub total_items: i32,
    #[serde(default)]
    pub kind: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Volume {
    pub id: String,
    #[serde(rename = "volumeInfo")]
    pub volume_info: VolumeInfo,
}

impl Volume {
    /// New helpers
    pub fn title(&self) -> String {
        let t = self.volume_info.title.trim();
        if t.is_empty() {
            "Untitled".to_string()
        } else {
            t.to_string()
        }
    }

    pub fn authors_display(&self) -> String {
        self.volume_info
            .authors
            .as_ref()
            .map(|v| v.join(", "))
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "Unknown Author".to_string())
    }

    // Compatibility helpers for existing call sites
    pub fn get_title(&self) -> String {
        self.title()
    }
    pub fn get_authors_string(&self) -> String {
        self.authors_display()
    }
    pub fn get_page_count(&self) -> Option<i32> {
        self.volume_info.page_count
    }
    pub fn get_description(&self) -> Option<String> {
        self.volume_info.description.clone()
    }

    // Prefer largest available image; fall back sanely.
    pub fn get_thumbnail_url(&self) -> Option<String> {
        let links = self.volume_info.image_links.as_ref()?;
        links
            .extra_large
            .clone()
            .or(links.large.clone())
            .or(links.medium.clone())
            .or(links.small.clone())
            .or(links.thumbnail.clone())
            .or(links.small_thumbnail.clone())
    }

    // Return ISBN_13 or ISBN_10 if present.
    // pub fn get_isbn(&self) -> Option<String> {
    //     let ids = self.volume_info.industry_identifiers.as_ref()?;
    //     ids.iter()
    //         .find(|id| matches!(id.id_type.as_deref(), Some("ISBN_13")))
    //         .and_then(|id| id.identifier.clone())
    //         .or_else(|| {
    //             ids.iter()
    //                 .find(|id| matches!(id.id_type.as_deref(), Some("ISBN_10")))
    //                 .and_then(|id| id.identifier.clone())
    //         })
    //         .or_else(|| ids.get(0).and_then(|id| id.identifier.clone()))
    // }

    // info.rs uses `.join(", ")` on this
    pub fn get_categories(&self) -> Vec<String> {
        self.volume_info.categories.clone().unwrap_or_default()
    }

    pub fn is_mature(&self) -> bool {
        self.volume_info
            .maturity_rating
            .as_ref()
            .map(|rating| rating == "MATURE")
            .unwrap_or(false)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct VolumeInfo {
    #[serde(default)]
    pub title: String,
    pub subtitle: Option<String>,
    pub authors: Option<Vec<String>>,
    pub publisher: Option<String>,
    #[serde(rename = "publishedDate")]
    pub published_date: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "industryIdentifiers")]
    pub industry_identifiers: Option<Vec<IndustryIdentifier>>,
    #[serde(rename = "pageCount")]
    pub page_count: Option<i32>,
    pub categories: Option<Vec<String>>,
    // #[serde(rename = "averageRating")]
    // pub average_rating: Option<f32>,
    // #[serde(rename = "ratingsCount")]
    // pub ratings_count: Option<i32>,
    #[serde(rename = "maturityRating")]
    pub maturity_rating: Option<String>, // "MATURE" OR "NOT_MATURE"
    #[serde(rename = "imageLinks")]
    pub image_links: Option<ImageLinks>,
    pub language: Option<String>,
    #[serde(rename = "previewLink")]
    pub preview_link: Option<String>,
    #[serde(rename = "infoLink")]
    pub info_link: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct IndustryIdentifier {
    #[serde(rename = "type")]
    pub id_type: Option<String>, // "ISBN_10", "ISBN_13", etc.
    pub identifier: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ImageLinks {
    #[serde(rename = "smallThumbnail")]
    pub small_thumbnail: Option<String>,
    pub thumbnail: Option<String>,
    pub small: Option<String>,
    pub medium: Option<String>,
    pub large: Option<String>,
    #[serde(rename = "extraLarge")]
    pub extra_large: Option<String>,
}

// truncate helper

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        let (prefix, truncated_bytes) = truncate_on_char_boundary(s, n);
        format!("{prefix}… ({} bytes truncated)", truncated_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_url_still_targets_google_books_volumes_endpoint() {
        let client = GoogleBooksClient::new(None);
        let volumes_url = client
            .base_url
            .join("volumes")
            .expect("joining volumes path should succeed");

        assert_eq!(
            volumes_url.as_str(),
            format!("{}/volumes", GOOGLE_BOOKS_API_BASE)
        );
    }

    #[test]
    fn custom_base_url_without_trailing_slash_is_normalized() {
        let client = GoogleBooksClient::new_with_base_url(None, "http://example.com/books/v1");

        let volumes_url = client
            .base_url
            .join("volumes")
            .expect("joining volumes path should succeed");

        assert_eq!(volumes_url.as_str(), "http://example.com/books/v1/volumes");
    }
}
