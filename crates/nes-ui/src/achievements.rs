use std::{
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use nes_achievements_native::{Achievement, Client, Event, Game, Request, User};
use nes_core::Nes;
use reqwest::{
    blocking::Client as HttpClient,
    header::{CONTENT_TYPE, USER_AGENT},
};

const USER_AGENT_VALUE: &str = "CrabNes/0.1.0 (Windows) rcheevos/12.3.0";

struct HttpResult {
    id: u64,
    status: i32,
    body: Vec<u8>,
}

pub struct BadgeImage {
    pub url: String,
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
}

pub struct Manager {
    client: Client,
    http: Arc<HttpClient>,
    result_tx: Sender<HttpResult>,
    result_rx: Receiver<HttpResult>,
    badge_request_tx: Sender<String>,
    badge_result_rx: Receiver<BadgeImage>,
    memory: Vec<u8>,
}

impl Manager {
    pub fn new() -> Result<Self, String> {
        let client = Client::new().map_err(str::to_owned)?;
        let http = Arc::new(
            HttpClient::builder()
                .user_agent(USER_AGENT_VALUE)
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())?,
        );
        let (result_tx, result_rx) = mpsc::channel();
        let (badge_request_tx, badge_request_rx) = mpsc::channel::<String>();
        let (badge_result_tx, badge_result_rx) = mpsc::channel();
        let badge_http = Arc::clone(&http);
        thread::Builder::new()
            .name("achievement-badges".into())
            .spawn(move || {
                while let Ok(url) = badge_request_rx.recv() {
                    let decoded = badge_http
                        .get(&url)
                        .header(USER_AGENT, USER_AGENT_VALUE)
                        .send()
                        .and_then(reqwest::blocking::Response::error_for_status)
                        .and_then(|response| response.bytes())
                        .ok()
                        .and_then(|bytes| image::load_from_memory(&bytes).ok())
                        .map(|image| image.thumbnail(128, 128).to_rgba8());
                    if let Some(rgba) = decoded {
                        let size = [rgba.width() as usize, rgba.height() as usize];
                        if badge_result_tx
                            .send(BadgeImage {
                                url,
                                size,
                                rgba: rgba.into_raw(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            client,
            http,
            result_tx,
            result_rx,
            badge_request_tx,
            badge_result_rx,
            memory: vec![0; 0x1_0000],
        })
    }

    pub fn login_password(&mut self, username: &str, password: &str) -> Result<(), String> {
        self.client.login_password(username, password)
    }

    pub fn login_token(&mut self, username: &str, token: &str) -> Result<(), String> {
        self.client.login_token(username, token)
    }

    pub fn logout(&mut self) {
        self.client.logout();
    }

    pub fn load_game(&mut self, path: &Path, data: &[u8]) -> Result<(), String> {
        self.client.load_nes_game(&path.to_string_lossy(), data)
    }

    pub fn unload_game(&mut self) {
        self.client.unload_game();
    }

    pub fn do_frame(&mut self, nes: &Nes) {
        nes.copy_achievement_memory(&mut self.memory);
        self.client.set_memory(&self.memory);
        self.client.do_frame();
    }

    pub fn reset(&mut self) {
        self.client.reset();
    }

    pub fn pump(&mut self, idle: bool) -> Vec<Event> {
        while let Ok(result) = self.result_rx.try_recv() {
            self.client
                .complete_request(result.id, result.status, &result.body);
        }
        if idle {
            self.client.idle();
        }
        while let Some(request) = self.client.take_request() {
            self.dispatch(request);
        }
        let mut events = Vec::new();
        while let Some(event) = self.client.pop_event() {
            events.push(event);
        }
        events
    }

    fn dispatch(&self, request: Request) {
        let http = Arc::clone(&self.http);
        let sender = self.result_tx.clone();
        thread::spawn(move || {
            let id = request.id;
            let response = if let Some(post_data) = request.post_data {
                http.post(&request.url)
                    .header(USER_AGENT, USER_AGENT_VALUE)
                    .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
                    .body(post_data)
                    .send()
            } else {
                http.get(&request.url)
                    .header(USER_AGENT, USER_AGENT_VALUE)
                    .send()
            };
            let result = match response {
                Ok(response) => {
                    let status = i32::from(response.status().as_u16());
                    match response.bytes() {
                        Ok(body) => HttpResult {
                            id,
                            status,
                            body: body.to_vec(),
                        },
                        Err(error) => HttpResult {
                            id,
                            status: 0,
                            body: error.to_string().into_bytes(),
                        },
                    }
                }
                Err(error) => HttpResult {
                    id,
                    status: 0,
                    body: error.to_string().into_bytes(),
                },
            };
            let _ = sender.send(result);
        });
    }

    pub fn user(&self) -> Option<User> {
        self.client.user()
    }

    pub fn game(&self) -> Option<Game> {
        self.client.game()
    }

    pub fn achievements(&mut self) -> Vec<Achievement> {
        self.client.achievements()
    }

    pub fn is_game_loaded(&self) -> bool {
        self.client.is_game_loaded()
    }

    pub fn is_hardcore(&self) -> bool {
        self.client.is_hardcore()
    }

    pub fn request_badge(&self, url: String) {
        let _ = self.badge_request_tx.send(url);
    }

    pub fn take_badge_images(&self) -> Vec<BadgeImage> {
        self.badge_result_rx.try_iter().collect()
    }
}
