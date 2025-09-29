// src/google_books_cache.rs
use anyhow::{Context, Result};
use moka::future::Cache;
use sha2::{Digest, Sha256};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::google_books::{build_search_query, GoogleBooksClient, Volume};
use crate::util::truncate_on_char_boundary;

/// Cache configuration constants
const VOLUME_CACHE_TTL_DAYS: u64 = 7; // 7 days for individual volumes
const SEARCH_CACHE_TTL_DAYS: u64 = 1; // 1 day for search results
const TOTAL_CACHE_SIZE_MB: u64 = 256; // Total cache size in MB
const VOLUME_CACHE_RATIO: f64 = 0.7; // 70% for volume cache
const SEARCH_CACHE_RATIO: f64 = 0.3; // 30% for search cache
const BYTES_PER_MB: u64 = 1024 * 1024;

/// Size estimation for cache entries (approximate bytes)
const ESTIMATED_VOLUME_SIZE: u32 = 2048; // ~2KB per volume (reduced to essentials)
const ESTIMATED_SEARCH_RESULT_SIZE: u32 = 2048; // ~2KB per cached volume when storing full result sets

/// Cached wrapper around GoogleBooksClient
#[derive(Clone)]
pub struct CachedGoogleBooksClient {
    client: GoogleBooksClient,
    search_cache: Cache<String, Arc<Vec<Volume>>>,
    volume_cache: Cache<String, Arc<Volume>>,
    stats: Arc<CacheStats>,
}

/// Statistics for monitoring cache performance
#[derive(Debug, Default)]
pub struct CacheStats {
    search_hits: std::sync::atomic::AtomicU64,
    search_misses: std::sync::atomic::AtomicU64,
    volume_hits: std::sync::atomic::AtomicU64,
    volume_misses: std::sync::atomic::AtomicU64,
    api_calls: std::sync::atomic::AtomicU64,
}

impl CacheStats {
    pub fn log_stats(&self) {
        use std::sync::atomic::Ordering;
        let search_hits = self.search_hits.load(Ordering::Relaxed);
        let search_misses = self.search_misses.load(Ordering::Relaxed);
        let volume_hits = self.volume_hits.load(Ordering::Relaxed);
        let volume_misses = self.volume_misses.load(Ordering::Relaxed);
        let api_calls = self.api_calls.load(Ordering::Relaxed);

        let search_total = search_hits + search_misses;
        let volume_total = volume_hits + volume_misses;

        if search_total > 0 {
            let search_hit_rate = (search_hits as f64 / search_total as f64) * 100.0;
            println!(
                "📊 Cache Stats - Search: {:.1}% hit rate ({}/{} hits)",
                search_hit_rate, search_hits, search_total
            );
        }

        if volume_total > 0 {
            let volume_hit_rate = (volume_hits as f64 / volume_total as f64) * 100.0;
            println!(
                "📊 Cache Stats - Volume: {:.1}% hit rate ({}/{} hits)",
                volume_hit_rate, volume_hits, volume_total
            );
        }

        println!("📊 Total Google Books API calls: {}", api_calls);
    }
}

impl CachedGoogleBooksClient {
    pub fn new(api_key: Option<String>) -> Self {
        // Calculate size allocation for each cache
        let total_bytes = TOTAL_CACHE_SIZE_MB * BYTES_PER_MB;
        let volume_cache_bytes = (total_bytes as f64 * VOLUME_CACHE_RATIO) as u64;
        let search_cache_bytes = (total_bytes as f64 * SEARCH_CACHE_RATIO) as u64;

        // Create search cache with size-based eviction
        let search_cache = Cache::builder()
            .max_capacity(search_cache_bytes)
            .weigher(|_key: &String, value: &Arc<Vec<Volume>>| -> u32 {
                // Search caches store the full result set for a query, so scale weight by
                // the number of cached volumes to keep usage within budget.
                (value.len() as u32 * ESTIMATED_SEARCH_RESULT_SIZE).min(u32::MAX)
            })
            .time_to_live(Duration::from_secs(SEARCH_CACHE_TTL_DAYS * 24 * 60 * 60))
            .time_to_idle(Duration::from_secs(12 * 60 * 60)) // Evict if not accessed for 12 hours
            .build();

        // Create volume cache with size-based eviction
        let volume_cache = Cache::builder()
            .max_capacity(volume_cache_bytes)
            .weigher(|_key: &String, _value: &Arc<Volume>| -> u32 { ESTIMATED_VOLUME_SIZE })
            .time_to_live(Duration::from_secs(VOLUME_CACHE_TTL_DAYS * 24 * 60 * 60))
            .time_to_idle(Duration::from_secs(24 * 60 * 60)) // Evict if not accessed for 24 hours
            .build();

        Self {
            client: GoogleBooksClient::new(api_key),
            search_cache,
            volume_cache,
            stats: Arc::new(CacheStats::default()),
        }
    }

    // Generate a stable cache key for search operations
    fn generate_search_key(
        query_type: &str,
        query: &str,
        author: Option<&str>,
        max_results: Option<u32>,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(query_type.as_bytes());
        hasher.update(b"|");
        hasher.update(query.to_lowercase().as_bytes());

        if let Some(a) = author {
            hasher.update(b"|author:");
            hasher.update(a.to_lowercase().as_bytes());
        }

        if let Some(max) = max_results {
            hasher.update(b"|max:");
            hasher.update(max.to_string().as_bytes());
        }

        hex::encode(hasher.finalize())
    }

    fn generate_combined_search_key(query: &str, max_results: Option<u32>) -> String {
        Self::generate_search_key("combined", query, None, max_results)
    }

    // Clean a Volume to only include necessary fields for caching
    fn clean_volume_for_cache(volume: &Volume) -> Volume {
        // Create a minimal version with only the fields we actually use
        let mut cleaned = volume.clone();

        // Clear large fields
        cleaned.volume_info.description = cleaned.volume_info.description.as_ref().map(|d| {
            if d.len() > 500 {
                let (prefix, _) = truncate_on_char_boundary(d, 497);
                format!("{prefix}...")
            } else {
                d.clone()
            }
        });

        cleaned
    }

    async fn fetch_and_cache_search_results<Fut>(
        &self,
        cache_key: String,
        fetch_results: Fut,
    ) -> Result<Vec<Volume>>
    where
        Fut: Future<Output = Result<Vec<Volume>>>,
    {
        // Check cache first
        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        // Cache miss - fetch from API
        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let results = fetch_results.await?;

        let results_to_cache: Vec<Volume> = results
            .iter()
            .map(|v| Self::clean_volume_for_cache(v))
            .collect();

        if !results_to_cache.is_empty() {
            let arc_results = Arc::new(results_to_cache);
            self.search_cache
                .insert(cache_key, Arc::clone(&arc_results))
                .await;

            // Also cache individual volumes for future direct lookups
            for volume in arc_results.iter() {
                let volume_key = format!("volume:{}", volume.id);
                self.volume_cache
                    .insert(volume_key, Arc::new(volume.clone()))
                    .await;
            }
        }

        Ok(results)
    }

    // Search books by title with caching
    pub async fn search_books(
        &self,
        title: &str,
        author: Option<&str>,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let cache_key = Self::generate_search_key("title", title, author, max_results);

        self.fetch_and_cache_search_results(
            cache_key,
            self.client.search_books(title, author, max_results),
        )
        .await
    }

    // Search by ISBN with caching
    pub async fn search_by_isbn(&self, isbn: &str) -> Result<Option<Volume>> {
        let cache_key = Self::generate_search_key("isbn", isbn, None, None);

        // Check search cache
        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok(cached.first().cloned());
        }

        // Cache miss - fetch from API
        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let result = self.client.search_by_isbn(isbn).await?;

        // Cache the result
        if let Some(ref volume) = result {
            let cleaned = Self::clean_volume_for_cache(volume);
            let arc_results = Arc::new(vec![cleaned.clone()]);
            self.search_cache.insert(cache_key, arc_results).await;

            // Also cache the individual volume
            let volume_key = format!("volume:{}", volume.id);
            self.volume_cache
                .insert(volume_key, Arc::new(cleaned))
                .await;
        } else {
            // Cache empty result to avoid repeated failed lookups
            self.search_cache.insert(cache_key, Arc::new(vec![])).await;
        }

        Ok(result)
    }

    // Search by author with caching, keeping as many results as requested (up to 40)
    pub async fn search_by_author(
        &self,
        author: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let cache_key = Self::generate_search_key("author", author, None, max_results);

        // Check cache first
        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        // Cache miss - fetch from API
        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let results = self.client.search_by_author(author, max_results).await?;

        let effective_max = max_results.unwrap_or(10).min(40) as usize;

        // Cache as many results as requested (up to Google's 40 result cap)
        let results_to_cache: Vec<Volume> = results
            .iter()
            .take(effective_max)
            .map(|v| Self::clean_volume_for_cache(v))
            .collect();

        if !results_to_cache.is_empty() {
            let arc_results = Arc::new(results_to_cache);
            self.search_cache
                .insert(cache_key, Arc::clone(&arc_results))
                .await;

            // Cache individual volumes
            for volume in arc_results.iter() {
                let volume_key = format!("volume:{}", volume.id);
                self.volume_cache
                    .insert(volume_key, Arc::new(volume.clone()))
                    .await;
            }
        } else {
            self.search_cache.insert(cache_key, Arc::new(vec![])).await;
        }

        // Return full results
        Ok(results)
    }

    pub async fn search_by_genre(
        &self,
        genre: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let cache_key = Self::generate_search_key("genre", genre, None, max_results);

        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let results = self.client.search_by_genre(genre, max_results).await?;

        let effective_max = max_results.unwrap_or(10).min(40) as usize;
        let results_to_cache: Vec<Volume> = results
            .iter()
            .take(effective_max)
            .map(|v| Self::clean_volume_for_cache(v))
            .collect();

        if !results_to_cache.is_empty() {
            let arc_results = Arc::new(results_to_cache);
            self.search_cache
                .insert(cache_key, Arc::clone(&arc_results))
                .await;

            for volume in arc_results.iter() {
                let volume_key = format!("volume:{}", volume.id);
                self.volume_cache
                    .insert(volume_key, Arc::new(volume.clone()))
                    .await;
            }
        } else {
            self.search_cache.insert(cache_key, Arc::new(vec![])).await;
        }

        Ok(results)
    }

    pub async fn search_by_publisher(
        &self,
        publisher: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let cache_key = Self::generate_search_key("publisher", publisher, None, max_results);

        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let results = self
            .client
            .search_by_publisher(publisher, max_results)
            .await?;

        let effective_max = max_results.unwrap_or(10).min(40) as usize;
        let results_to_cache: Vec<Volume> = results
            .iter()
            .take(effective_max)
            .map(|v| Self::clean_volume_for_cache(v))
            .collect();

        if !results_to_cache.is_empty() {
            let arc_results = Arc::new(results_to_cache);
            self.search_cache
                .insert(cache_key, Arc::clone(&arc_results))
                .await;

            for volume in arc_results.iter() {
                let volume_key = format!("volume:{}", volume.id);
                self.volume_cache
                    .insert(volume_key, Arc::new(volume.clone()))
                    .await;
            }
        } else {
            self.search_cache.insert(cache_key, Arc::new(vec![])).await;
        }

        Ok(results)
    }

    pub async fn search(
        &self,
        query: Option<&str>,
        author: Option<&str>,
        genre: Option<&str>,
        publisher: Option<&str>,
        max_results: Option<u32>,
    ) -> Result<Vec<Volume>> {
        let Some(combined_query) = build_search_query(query, author, genre, publisher) else {
            return Ok(vec![]);
        };

        let cache_key = Self::generate_combined_search_key(&combined_query, max_results);

        if let Some(cached) = self.search_cache.get(&cache_key).await {
            self.stats
                .search_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        self.stats
            .search_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let results = self
            .client
            .search(query, author, genre, publisher, max_results)
            .await?;

        let effective_max = max_results.unwrap_or(10).min(40) as usize;
        let results_to_cache: Vec<Volume> = results
            .iter()
            .take(effective_max)
            .map(|v| Self::clean_volume_for_cache(v))
            .collect();

        if !results_to_cache.is_empty() {
            let arc_results = Arc::new(results_to_cache);
            self.search_cache
                .insert(cache_key, Arc::clone(&arc_results))
                .await;

            for volume in arc_results.iter() {
                let volume_key = format!("volume:{}", volume.id);
                self.volume_cache
                    .insert(volume_key, Arc::new(volume.clone()))
                    .await;
            }
        } else {
            self.search_cache.insert(cache_key, Arc::new(vec![])).await;
        }

        Ok(results)
    }

    // Get a specific volume by ID with caching
    pub async fn get_volume(&self, volume_id: &str) -> Result<Volume> {
        let cache_key = format!("volume:{}", volume_id);

        // Check cache first
        if let Some(cached) = self.volume_cache.get(&cache_key).await {
            self.stats
                .volume_hits
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Ok((*cached).clone());
        }

        // Cache miss - fetch from API
        self.stats
            .volume_misses
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.stats
            .api_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let volume = self.client.get_volume(volume_id).await.map_err(|err| {
            eprintln!("Error fetching Google Books volume {}: {err:?}", volume_id);
            err.context(format!(
                "Google Books volume fetch failed for ID {volume_id}"
            ))
        })?;

        // Cache the cleaned volume
        let cleaned = Self::clean_volume_for_cache(&volume);
        self.volume_cache.insert(cache_key, Arc::new(cleaned)).await;

        Ok(volume)
    }

    // Batch fetch multiple volumes with parallel requests (respecting rate limits)
    pub async fn get_volumes_batch(&self, volume_ids: &[String]) -> Vec<Result<Volume>> {
        use futures::future::join_all;
        use tokio::time::sleep;

        // Check cache first and separate hits from misses
        let mut results = Vec::with_capacity(volume_ids.len());
        let mut cache_misses = Vec::new();

        for (idx, volume_id) in volume_ids.iter().enumerate() {
            let cache_key = format!("volume:{}", volume_id);
            if let Some(cached) = self.volume_cache.get(&cache_key).await {
                self.stats
                    .volume_hits
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                results.push((idx, Ok((*cached).clone())));
            } else {
                cache_misses.push((idx, volume_id.clone()));
            }
        }

        // Fetch missing volumes in batches to respect rate limits (100/minute)
        const BATCH_SIZE: usize = 10;
        const BATCH_DELAY_MS: u64 = 1000; // 1 second between batches

        for chunk in cache_misses.chunks(BATCH_SIZE) {
            let futures: Vec<_> = chunk
                .iter()
                .map(|(idx, volume_id)| {
                    let id = volume_id.clone();
                    let cache = self.clone();
                    async move { (*idx, cache.get_volume(&id).await) }
                })
                .collect();

            let batch_results = join_all(futures).await;
            results.extend(batch_results);

            // Add delay between batches to respect rate limits
            if cache_misses.len() > BATCH_SIZE {
                sleep(Duration::from_millis(BATCH_DELAY_MS)).await;
            }
        }

        // Sort results back to original order
        results.sort_by_key(|(idx, _)| *idx);
        results.into_iter().map(|(_, result)| result).collect()
    }

    // pub async fn clear_caches(&self) {
    //     self.search_cache.invalidate_all();
    //     self.volume_cache.invalidate_all();
    //     self.search_cache.run_pending_tasks().await;
    //     self.volume_cache.run_pending_tasks().await;
    //     println!("All Google Books caches cleared");
    // }

    // // Get cache statistics
    // pub fn get_stats(&self) -> &CacheStats {
    //     &self.stats
    // }

    // Log current cache sizes
    pub async fn log_cache_info(&self) {
        let search_size = self.search_cache.entry_count();
        let volume_size = self.volume_cache.entry_count();
        let search_weight = self.search_cache.weighted_size();
        let volume_weight = self.volume_cache.weighted_size();

        println!(
            "📚 Cache sizes - Search: {} entries (~{:.2} MB), Volume: {} entries (~{:.2} MB)",
            search_size,
            search_weight as f64 / BYTES_PER_MB as f64,
            volume_size,
            volume_weight as f64 / BYTES_PER_MB as f64
        );
        self.stats.log_stats();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google_books::{SearchResponse, VolumeInfo};
    use anyhow::Result;
    use std::net::TcpListener;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    fn make_volume(id: &str) -> Volume {
        Volume {
            id: id.to_string(),
            volume_info: VolumeInfo {
                title: format!("Test Volume {id}"),
                subtitle: None,
                authors: Some(vec!["Tester".to_string()]),
                publisher: None,
                published_date: None,
                description: Some(format!("Description for {id}")),
                industry_identifiers: None,
                page_count: Some(123),
                categories: None,
                maturity_rating: None,
                image_links: None,
                language: Some("en".to_string()),
                preview_link: None,
                info_link: None,
            },
        }
    }

    #[tokio::test]
    async fn caches_full_search_results_and_seeds_volume_cache() -> Result<()> {
        let client = CachedGoogleBooksClient::new(None);
        let title = "Cache test";
        let author = Some("Tester");
        let max_results = Some(5);
        let cache_key =
            CachedGoogleBooksClient::generate_search_key("title", title, author, max_results);

        let volumes = vec![
            make_volume("vol1"),
            make_volume("vol2"),
            make_volume("vol3"),
        ];
        let fetched_volumes = volumes.clone();
        client
            .fetch_and_cache_search_results(cache_key.clone(), async move { Ok(fetched_volumes) })
            .await?;

        let cached_entry = client
            .search_cache
            .get(&cache_key)
            .await
            .expect("cached results missing");
        assert_eq!(cached_entry.len(), volumes.len());

        for volume in &volumes {
            let volume_key = format!("volume:{}", volume.id);
            let cached_volume = client.volume_cache.get(&volume_key).await;
            assert!(
                cached_volume.is_some(),
                "volume {} was not seeded into the volume cache",
                volume.id
            );
        }

        let repeat_call = client.search_books(title, author, max_results).await?;
        assert_eq!(repeat_call.len(), volumes.len());
        let returned_ids: Vec<_> = repeat_call.iter().map(|v| v.id.as_str()).collect();
        let expected_ids: Vec<_> = volumes.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(returned_ids, expected_ids);

        assert_eq!(
            client.stats.search_hits.load(Ordering::Relaxed),
            1,
            "cache hit counter should increment for the repeat call"
        );

        Ok(())
    }

    #[tokio::test]
    async fn author_search_caches_requested_result_count() -> Result<()> {
        use axum::{extract::State, routing::get, Json, Router};
        use tokio::net::TcpListener as TokioTcpListener;

        #[derive(Clone)]
        struct TestState {
            volumes: Arc<Vec<Volume>>,
            request_count: Arc<AtomicUsize>,
        }

        async fn handler(State(state): State<TestState>) -> Json<SearchResponse> {
            state.request_count.fetch_add(1, Ordering::Relaxed);
            Json(SearchResponse {
                items: Some(state.volumes.as_ref().clone()),
                total_items: state.volumes.len() as i32,
                kind: String::new(),
            })
        }

        let requested_results = 10usize;
        let volumes: Vec<_> = (0..requested_results)
            .map(|i| make_volume(&format!("author-vol-{i}")))
            .collect();
        let volumes_arc = Arc::new(volumes.clone());
        let request_count = Arc::new(AtomicUsize::new(0));

        let state = TestState {
            volumes: Arc::clone(&volumes_arc),
            request_count: Arc::clone(&request_count),
        };

        let std_listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = std_listener.local_addr()?;
        std_listener.set_nonblocking(true)?;
        let listener = TokioTcpListener::from_std(std_listener)?;

        let app = Router::new()
            .route("/books/v1/volumes", get(handler))
            .with_state(state);

        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });

        let mut cached_client = CachedGoogleBooksClient::new(None);
        cached_client.client =
            GoogleBooksClient::new_with_base_url(None, &format!("http://{}/books/v1/", addr));

        let max_results = Some(requested_results as u32);

        let first_call = cached_client
            .search_by_author("Tester", max_results)
            .await?;
        assert_eq!(first_call.len(), volumes.len());

        let second_call = cached_client
            .search_by_author("Tester", max_results)
            .await?;
        assert_eq!(second_call.len(), volumes.len());

        assert_eq!(
            request_count.load(Ordering::Relaxed),
            1,
            "expected only one HTTP request thanks to caching",
        );

        let cache_key =
            CachedGoogleBooksClient::generate_search_key("author", "Tester", None, max_results);
        let cached_entry = cached_client
            .search_cache
            .get(&cache_key)
            .await
            .expect("cached author results missing");
        assert_eq!(cached_entry.len(), volumes.len());

        server_handle.abort();

        Ok(())
    }

    #[tokio::test]
    async fn caches_genre_search_results() -> Result<()> {
        use axum::{extract::State, routing::get, Json, Router};
        use std::net::TcpListener;
        use tokio::net::TcpListener as TokioTcpListener;

        #[derive(Clone)]
        struct TestState {
            volumes: Arc<Vec<Volume>>,
            request_count: Arc<AtomicUsize>,
        }

        async fn handler(State(state): State<TestState>) -> Json<SearchResponse> {
            state.request_count.fetch_add(1, Ordering::Relaxed);
            Json(SearchResponse {
                items: Some(state.volumes.as_ref().clone()),
                total_items: state.volumes.len() as i32,
                kind: String::new(),
            })
        }

        let requested_results = 8usize;
        let volumes: Vec<_> = (0..requested_results)
            .map(|i| make_volume(&format!("genre-vol-{i}")))
            .collect();
        let volumes_arc = Arc::new(volumes.clone());
        let request_count = Arc::new(AtomicUsize::new(0));

        let state = TestState {
            volumes: Arc::clone(&volumes_arc),
            request_count: Arc::clone(&request_count),
        };

        let std_listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = std_listener.local_addr()?;
        std_listener.set_nonblocking(true)?;
        let listener = TokioTcpListener::from_std(std_listener)?;

        let app = Router::new()
            .route("/books/v1/volumes", get(handler))
            .with_state(state);

        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });

        let mut cached_client = CachedGoogleBooksClient::new(None);
        cached_client.client =
            GoogleBooksClient::new_with_base_url(None, &format!("http://{}/books/v1/", addr));

        let max_results = Some(requested_results as u32);

        let first_call = cached_client
            .search_by_genre("Fantasy", max_results)
            .await?;
        assert_eq!(first_call.len(), volumes.len());

        let second_call = cached_client
            .search_by_genre("Fantasy", max_results)
            .await?;
        assert_eq!(second_call.len(), volumes.len());

        assert_eq!(
            request_count.load(Ordering::Relaxed),
            1,
            "expected only one HTTP request thanks to caching",
        );

        let cache_key =
            CachedGoogleBooksClient::generate_search_key("genre", "Fantasy", None, max_results);
        let cached_entry = cached_client
            .search_cache
            .get(&cache_key)
            .await
            .expect("cached genre results missing");
        assert_eq!(cached_entry.len(), volumes.len());

        server_handle.abort();

        Ok(())
    }

    #[tokio::test]
    async fn caches_publisher_search_results() -> Result<()> {
        use axum::{extract::State, routing::get, Json, Router};
        use std::net::TcpListener;
        use tokio::net::TcpListener as TokioTcpListener;

        #[derive(Clone)]
        struct TestState {
            volumes: Arc<Vec<Volume>>,
            request_count: Arc<AtomicUsize>,
        }

        async fn handler(State(state): State<TestState>) -> Json<SearchResponse> {
            state.request_count.fetch_add(1, Ordering::Relaxed);
            Json(SearchResponse {
                items: Some(state.volumes.as_ref().clone()),
                total_items: state.volumes.len() as i32,
                kind: String::new(),
            })
        }

        let requested_results = 6usize;
        let volumes: Vec<_> = (0..requested_results)
            .map(|i| make_volume(&format!("publisher-vol-{i}")))
            .collect();
        let volumes_arc = Arc::new(volumes.clone());
        let request_count = Arc::new(AtomicUsize::new(0));

        let state = TestState {
            volumes: Arc::clone(&volumes_arc),
            request_count: Arc::clone(&request_count),
        };

        let std_listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = std_listener.local_addr()?;
        std_listener.set_nonblocking(true)?;
        let listener = TokioTcpListener::from_std(std_listener)?;

        let app = Router::new()
            .route("/books/v1/volumes", get(handler))
            .with_state(state);

        let server_handle = tokio::spawn(async move {
            let _ = axum::serve(listener, app.into_make_service()).await;
        });

        let mut cached_client = CachedGoogleBooksClient::new(None);
        cached_client.client =
            GoogleBooksClient::new_with_base_url(None, &format!("http://{}/books/v1/", addr));

        let max_results = Some(requested_results as u32);

        let first_call = cached_client
            .search_by_publisher("Test Publisher", max_results)
            .await?;
        assert_eq!(first_call.len(), volumes.len());

        let second_call = cached_client
            .search_by_publisher("Test Publisher", max_results)
            .await?;
        assert_eq!(second_call.len(), volumes.len());

        assert_eq!(
            request_count.load(Ordering::Relaxed),
            1,
            "expected only one HTTP request thanks to caching",
        );

        let cache_key = CachedGoogleBooksClient::generate_search_key(
            "publisher",
            "Test Publisher",
            None,
            max_results,
        );
        let cached_entry = cached_client
            .search_cache
            .get(&cache_key)
            .await
            .expect("cached publisher results missing");
        assert_eq!(cached_entry.len(), volumes.len());

        server_handle.abort();

        Ok(())
    }
}
