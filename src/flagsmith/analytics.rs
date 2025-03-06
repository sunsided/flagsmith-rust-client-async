use log::{debug, warn};
use reqwest::header::HeaderMap;
use serde_json;
use std::collections::HashMap;
use std::sync::mpsc::{SyncSender, TryRecvError};
use std::sync::{Arc, mpsc};
use tokio::sync::RwLock;

static ANALYTICS_TIMER_IN_MILLI: u64 = 10 * 1000;

#[derive(Clone, Debug)]
pub struct AnalyticsProcessor {
    pub tx: SyncSender<u32>,
    _analytics_data: Arc<RwLock<HashMap<u32, u32>>>,
}

impl AnalyticsProcessor {
    pub fn new(
        api_url: String,
        headers: HeaderMap,
        timeout: std::time::Duration,
        timer: Option<u64>,
    ) -> Self {
        let (tx, rx) = mpsc::sync_channel::<u32>(10);
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(timeout)
            .build()
            .unwrap();
        let analytics_endpoint = format!("{}analytics/flags/", api_url);
        let timer = timer.unwrap_or(ANALYTICS_TIMER_IN_MILLI);

        let analytics_data_arc: Arc<RwLock<HashMap<u32, u32>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let analytics_data_locked = Arc::clone(&analytics_data_arc);
        tokio::spawn(async move {
            let mut last_flushed = chrono::Utc::now();
            loop {
                let data = rx.try_recv();
                let mut analytics_data = analytics_data_locked.write().await;
                match data {
                    // Update the analytics data with feature_id received
                    Ok(feature_id) => {
                        analytics_data
                            .entry(feature_id)
                            .and_modify(|e| *e += 1)
                            .or_insert(1);
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        debug!("Shutting down analytics thread ");
                        break;
                    }
                };
                if (chrono::Utc::now() - last_flushed).num_milliseconds() > timer as i64 {
                    flush(&client, &analytics_data, &analytics_endpoint).await;
                    analytics_data.clear();
                    last_flushed = chrono::Utc::now();
                }
            }
        });

        AnalyticsProcessor {
            tx,
            _analytics_data: Arc::clone(&analytics_data_arc),
        }
    }
    pub fn track_feature(&self, feature_id: u32) {
        self.tx.send(feature_id).unwrap();
    }
}

async fn flush(
    client: &reqwest::Client,
    analytics_data: &HashMap<u32, u32>,
    analytics_endpoint: &str,
) {
    if analytics_data.is_empty() {
        return;
    }
    let body = serde_json::to_string(&analytics_data).unwrap();
    let resp = client.post(analytics_endpoint).body(body).send();
    if resp.await.is_err() {
        warn!("Failed to send analytics data");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use reqwest::header;

    #[tokio::test]
    async fn track_feature_updates_analytics_data() {
        // Given
        let feature_1 = 1;
        let processor = AnalyticsProcessor::new(
            "http://localhost".to_string(),
            header::HeaderMap::new(),
            std::time::Duration::from_secs(10),
            Some(10000),
        );
        // Now, let's make tracking calls
        processor.track_feature(feature_1);
        processor.track_feature(feature_1);
        // Wait a little for it to receive the message
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let analytics_data = processor._analytics_data.read().await;
        // Then, verify that analytics_data was updated correctly
        assert_eq!(analytics_data[&feature_1], 2);
    }

    #[tokio::test]
    async fn test_analytics_processor() {
        // Given
        let feature_1 = 1;
        let feature_2 = 2;
        let server = MockServer::start();
        let first_invocation_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/analytics/flags/")
                .header("X-Environment-Key", "ser.UiYoRr6zUjiFBUXaRwo7b5")
                .json_body(serde_json::json!({feature_1.to_string():10, feature_2.to_string():10}));
            then.status(200).header("content-type", "application/json");
        });
        let mut headers = header::HeaderMap::new();
        headers.insert(
            "X-Environment-Key",
            header::HeaderValue::from_str("ser.UiYoRr6zUjiFBUXaRwo7b5").unwrap(),
        );
        let url = server.url("/api/v1/");

        let processor =
            AnalyticsProcessor::new(url, headers, std::time::Duration::from_secs(10), Some(10));
        // Now, let's update the analytics data
        let mut analytics_data = processor._analytics_data.write().await;
        analytics_data.insert(1, 10);
        analytics_data.insert(2, 10);
        // drop the analytics data to release the lock
        drop(analytics_data);
        // Next, let's sleep a little to let the processor flush the data
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Finally, let's assert that the mock was called
        first_invocation_mock.assert();
        // and, analytics data is now empty
        let analytics_data = processor._analytics_data.read().await;
        assert!(analytics_data.is_empty())
    }
}
