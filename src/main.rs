use anyhow::{Context, Result};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub struct DownloadConfig {
    pub url: String,
    pub save_path: PathBuf,
    pub threads: usize,
    pub report_progress: bool,
}

pub struct Downloader {
    config: DownloadConfig,
    temp_path: PathBuf,
    total_size: u64,
    downloaded: Arc<AtomicU64>,
    completed: Arc<AtomicBool>,
    supports_range: bool,
}

impl Downloader {
    pub fn new(config: DownloadConfig) -> Result<Self> {
        // 创建临时文件路径
        let temp_path = config.save_path.with_extension("download");

        // 获取文件信息
        let client = reqwest::blocking::Client::new();
        let response = client.head(&config.url).send()?;

        // 获取文件大小
        let total_size = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .context("Missing Content-Length header")?;

        // 检查范围请求支持
        let supports_range = response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .map(|v| v == "bytes")
            .unwrap_or(false);

        Ok(Self {
            config,
            temp_path,
            total_size,
            downloaded: Arc::new(AtomicU64::new(0)),
            completed: Arc::new(AtomicBool::new(false)),
            supports_range,
        })
    }

    pub fn start(&self) -> Result<()> {
        // 创建临时文件
        File::create(&self.temp_path)?;

        // 实际线程数处理
        let threads = if self.supports_range {
            self.config.threads
        } else {
            1
        };

        // 启动下载线程
        let mut handles = vec![];
        for i in 0..threads {
            let handle = self.spawn_download_thread(i, threads)?;
            handles.push(handle);
        }

        // 启动进度报告线程
        let progress_handle = if self.config.report_progress {
            Some(self.spawn_progress_thread())
        } else {
            None
        };

        // 等待下载完成
        for handle in handles {
            handle.join().map_err(|_| anyhow::anyhow!("Thread panicked"))??;
        }

        // 完成处理
        self.completed.store(true, Ordering::Relaxed);
        if let Some(h) = progress_handle {
            h.join().unwrap();
        }

        // 重命名临时文件
        fs::rename(&self.temp_path, &self.config.save_path)?;

        Ok(())
    }

    fn spawn_download_thread(&self, thread_id: usize, total_threads: usize) -> Result<thread::JoinHandle<Result<()>>> {
        let url = self.config.url.clone();
        let temp_path = self.temp_path.clone();
        let downloaded = self.downloaded.clone();
        let supports_range = self.supports_range;

        let (start, end) = if supports_range {
            let chunk_size = self.total_size / total_threads as u64;
            let start = thread_id as u64 * chunk_size;
            let end = if thread_id == total_threads - 1 {
                self.total_size - 1
            } else {
                start + chunk_size - 1
            };
            (Some(start), Some(end))
        } else {
            (None, None)
        };

        Ok(thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let mut request = client.get(&url);

            if let (Some(s), Some(e)) = (start, end) {
                request = request.header("Range", format!("bytes={}-{}", s, e));
            }

            let mut response = request.send()?;
            let mut file = OpenOptions::new().write(true).open(temp_path)?;

            let mut buffer = [0u8; 1024 * 1024]; // 1MB buffer
            let mut position = start.unwrap_or(0);

            loop {
                let bytes_read = response.read(&mut buffer)?;
                if bytes_read == 0 {
                    break;
                }

                file.seek(SeekFrom::Start(position))?;
                file.write_all(&buffer[..bytes_read])?;

                downloaded.fetch_add(bytes_read as u64, Ordering::Relaxed);
                position += bytes_read as u64;
            }

            Ok(())
        }))
    }

    fn spawn_progress_thread(&self) -> thread::JoinHandle<()> {
        let downloaded = self.downloaded.clone();
        let total_size = self.total_size;
        let completed = self.completed.clone();

        thread::spawn(move || {
            while !completed.load(Ordering::Relaxed) {
                let downloaded_bytes = downloaded.load(Ordering::Relaxed);
                let progress = (downloaded_bytes as f64 / total_size as f64) * 100.0;
                println!("{:.2}%", progress);
                thread::sleep(Duration::from_secs(1));
            }
        })
    }
}

impl Drop for Downloader {
    fn drop(&mut self) {
        if !self.completed.load(Ordering::Relaxed) {
            let _ = fs::remove_file(&self.temp_path);
        }
    }
}

// 使用示例
fn main() -> Result<()> {
    let config = DownloadConfig {
        url: "https://mirrors.sustech.edu.cn/Adoptium/17/jre/x64/windows/OpenJDK17U-jre_x64_windows_hotspot_17.0.14_7.zip".to_string(),
        save_path: PathBuf::from("d:/cpw/OpenJDK17U-jre_x64_windows_hotspot_17.0.14_7.zip"),
        threads: 1,
        report_progress: false,
    };

    let downloader = Downloader::new(config)?;
    downloader.start()?;
    Ok(())
}