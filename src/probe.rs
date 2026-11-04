use anyhow::Result;
use reqwest::{Client, redirect::Policy};
use serde::Serialize;
use std::time::Duration;
use tokio::{runtime::Runtime, task::JoinSet};

#[derive(Serialize)]
pub struct ProbeResult {
    pub url: String,
    pub ok: bool,
    pub detail: String,
}
pub struct Prober {
    client: Client,
    urls: Vec<String>,
    runtime: Runtime,
}
impl Prober {
    pub fn new(urls: &[String], timeout: Duration) -> Result<Self> {
        let urls = if urls.is_empty() {
            vec![
                "http://connect.rom.miui.com/generate_204".into(),
                "http://www.gstatic.com/generate_204".into(),
                "http://connectivitycheck.gstatic.com/generate_204".into(),
            ]
        } else {
            urls.to_vec()
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let client = Client::builder()
            .timeout(timeout)
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()?;
        Ok(Self {
            client,
            urls,
            runtime,
        })
    }
    fn launch(&self) -> JoinSet<(usize, ProbeResult)> {
        let mut tasks = JoinSet::new();
        for (index, url) in self.urls.iter().cloned().enumerate() {
            let client = self.client.clone();
            tasks.spawn(async move {
                let result = match client.get(&url).send().await {
                    Ok(r) => ProbeResult {
                        url,
                        ok: r.status() == 204,
                        detail: if r.status() == 204 {
                            "204".into()
                        } else {
                            r.status().to_string()
                        },
                    },
                    Err(e) => ProbeResult {
                        url,
                        ok: false,
                        detail: e.without_url().to_string(),
                    },
                };
                (index, result)
            });
        }
        tasks
    }
    pub fn check(&self) -> Vec<ProbeResult> {
        self.runtime.block_on(async {
            let mut tasks = self.launch();
            let mut results = Vec::new();
            while !tasks.is_empty() && !crate::shutdown::stopped() {
                if let Ok(Some(Ok(result))) =
                    tokio::time::timeout(Duration::from_millis(100), tasks.join_next()).await
                {
                    results.push(result);
                }
            }
            results.sort_by_key(|r| r.0);
            results.into_iter().map(|r| r.1).collect()
        })
    }
    pub fn online(&self) -> bool {
        self.runtime.block_on(async {
            // Dropping the task set cancels outstanding HTTP futures on success or shutdown.
            let mut tasks = self.launch();
            while !tasks.is_empty() && !crate::shutdown::stopped() {
                if let Ok(Some(Ok((_, result)))) =
                    tokio::time::timeout(Duration::from_millis(100), tasks.join_next()).await
                    && result.ok
                {
                    return true;
                }
            }
            false
        })
    }
}
