use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
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
use serde::{Deserialize, de::DeserializeOwned};

const USER_AGENT_VALUE: &str = "CrabNes/0.1.0 (Windows) rcheevos/12.3.0";
const RETROACHIEVEMENTS_API: &str = "https://retroachievements.org/dorequest.php";
const RETROACHIEVEMENTS_MEDIA: &str = "https://media.retroachievements.org";
const MAX_LIBRARY_ARTWORK_BYTES: usize = 32 * 1024 * 1024;

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

pub struct LibraryArtworkResult {
    pub path: PathBuf,
    pub image: Option<BadgeImage>,
}

pub struct LibraryArtworkLoader {
    request_tx: Sender<PathBuf>,
    result_rx: Receiver<LibraryArtworkResult>,
    pending: HashSet<PathBuf>,
}

impl LibraryArtworkLoader {
    pub fn new() -> Result<Self, String> {
        let http = http_client()?;
        let (request_tx, request_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, result_rx) = mpsc::channel();
        thread::Builder::new()
            .name("library-artwork".into())
            .spawn(move || {
                while let Ok(path) = request_rx.recv() {
                    let image = lookup_library_artwork(&http, &path).ok().flatten();
                    if result_tx
                        .send(LibraryArtworkResult { path, image })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            request_tx,
            result_rx,
            pending: HashSet::new(),
        })
    }

    pub fn request(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        if self.pending.insert(path.clone()) && self.request_tx.send(path.clone()).is_err() {
            self.pending.remove(&path);
        }
    }

    pub fn take_results(&mut self) -> Vec<LibraryArtworkResult> {
        let results: Vec<_> = self.result_rx.try_iter().collect();
        for result in &results {
            self.pending.remove(&result.path);
        }
        results
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
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
        let http = Arc::new(http_client()?);
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

fn http_client() -> Result<HttpClient, String> {
    HttpClient::builder()
        .user_agent(USER_AGENT_VALUE)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())
}

#[derive(Deserialize)]
struct ResolveHashResponse {
    #[serde(rename = "GameID", default)]
    game_id: u32,
}

#[derive(Deserialize)]
struct GameTitlesResponse {
    #[serde(rename = "Response", default)]
    games: Vec<GameTitle>,
}

#[derive(Deserialize)]
struct GameTitle {
    #[serde(rename = "ID")]
    id: u32,
    #[serde(rename = "ImageIcon", default)]
    image_icon: String,
    #[serde(rename = "ImageUrl", default)]
    image_url: String,
}

fn lookup_library_artwork(http: &HttpClient, path: &Path) -> Result<Option<BadgeImage>, String> {
    let rom = fs::read(path).map_err(|error| error.to_string())?;
    let hash = nes_achievements_native::hash_nes_game(&rom)
        .ok_or_else(|| "rcheevos could not hash the NES ROM".to_owned())?;
    let resolved: ResolveHashResponse =
        post_form_json(http, &[("r", "gameid"), ("m", hash.as_str())])?;
    let resolved_game_id = canonical_game_id(resolved.game_id);
    if resolved_game_id == 0 {
        return Ok(None);
    }
    let game_id = resolved_game_id.to_string();
    let titles: GameTitlesResponse =
        post_form_json(http, &[("r", "gameinfolist"), ("g", game_id.as_str())])?;
    let Some(game) = titles
        .games
        .into_iter()
        .find(|game| game.id == resolved_game_id)
    else {
        return Ok(None);
    };
    let Some(url) = game_artwork_url(&game) else {
        return Ok(None);
    };
    let response = http
        .get(&url)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_LIBRARY_ARTWORK_BYTES as u64)
    {
        return Err("library artwork is larger than 32 MiB".into());
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    if bytes.len() > MAX_LIBRARY_ARTWORK_BYTES {
        return Err("library artwork is larger than 32 MiB".into());
    }
    let rgba = image::load_from_memory(&bytes)
        .map_err(|error| error.to_string())?
        .thumbnail(256, 256)
        .to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok(Some(BadgeImage {
        url,
        size,
        rgba: rgba.into_raw(),
    }))
}

fn canonical_game_id(game_id: u32) -> u32 {
    // ResolveHash identifies known, unsupported revisions by adding this offset
    // to the compatible achievement set's game ID.
    const UNSUPPORTED_REVISION_OFFSET: u32 = 1_100_000_000;
    const UNSUPPORTED_REVISION_END: u32 = 1_200_000_000;

    if (UNSUPPORTED_REVISION_OFFSET + 1..UNSUPPORTED_REVISION_END).contains(&game_id) {
        game_id - UNSUPPORTED_REVISION_OFFSET
    } else {
        game_id
    }
}

fn post_form_json<T: DeserializeOwned>(
    http: &HttpClient,
    form: &[(&str, &str)],
) -> Result<T, String> {
    let response = http
        .post(RETROACHIEVEMENTS_API)
        .header(USER_AGENT, USER_AGENT_VALUE)
        .form(form)
        .send()
        .and_then(reqwest::blocking::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    serde_json::from_slice(&bytes).map_err(|error| error.to_string())
}

fn game_artwork_url(game: &GameTitle) -> Option<String> {
    if !game.image_url.is_empty() {
        return Some(
            if game.image_url.starts_with("http://") || game.image_url.starts_with("https://") {
                game.image_url.clone()
            } else {
                format!(
                    "{RETROACHIEVEMENTS_MEDIA}/{}",
                    game.image_url.trim_start_matches('/')
                )
            },
        );
    }
    let image_name = game
        .image_icon
        .rsplit('/')
        .next()
        .unwrap_or(&game.image_icon)
        .trim_end_matches(".png");
    (!image_name.is_empty()).then(|| format!("{RETROACHIEVEMENTS_MEDIA}/Images/{image_name}.png"))
}

#[cfg(test)]
mod tests {
    use super::canonical_game_id;

    #[test]
    fn keeps_regular_game_ids() {
        assert_eq!(canonical_game_id(1446), 1446);
    }

    #[test]
    fn unwraps_known_unsupported_revision_ids() {
        assert_eq!(canonical_game_id(1_100_001_446), 1446);
    }
}
