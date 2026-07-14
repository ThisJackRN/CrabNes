use std::{
    ffi::{CStr, CString, c_char, c_int, c_uchar, c_uint, c_ulonglong, c_void},
    marker::PhantomData,
    ptr::NonNull,
};

const URL_SIZE: usize = 2048;
const POST_SIZE: usize = 8192;
const TITLE_SIZE: usize = 160;
const MESSAGE_SIZE: usize = 320;
const TOKEN_SIZE: usize = 128;
const IMAGE_URL_SIZE: usize = 512;

#[repr(C)]
struct NativeClient(c_void);

#[repr(C)]
struct NativeRequest {
    id: c_ulonglong,
    has_post_data: c_int,
    url: [c_char; URL_SIZE],
    post_data: [c_char; POST_SIZE],
}

#[repr(C)]
struct NativeEvent {
    kind: c_int,
    result: c_int,
    id: c_uint,
    points: c_uint,
    title: [c_char; TITLE_SIZE],
    message: [c_char; MESSAGE_SIZE],
}

#[repr(C)]
struct NativeUser {
    score: c_uint,
    score_softcore: c_uint,
    username: [c_char; TITLE_SIZE],
    display_name: [c_char; TITLE_SIZE],
    token: [c_char; TOKEN_SIZE],
}

#[repr(C)]
struct NativeGame {
    id: c_uint,
    console_id: c_uint,
    title: [c_char; TITLE_SIZE],
    hash: [c_char; 40],
}

#[derive(Clone, Copy)]
#[repr(C)]
struct NativeAchievement {
    id: c_uint,
    points: c_uint,
    unlocked: c_uchar,
    bucket: c_uchar,
    measured_percent: f32,
    title: [c_char; TITLE_SIZE],
    description: [c_char; MESSAGE_SIZE],
    measured_progress: [c_char; 32],
    badge_url: [c_char; IMAGE_URL_SIZE],
    badge_locked_url: [c_char; IMAGE_URL_SIZE],
}

unsafe extern "C" {
    fn nes_ra_create() -> *mut NativeClient;
    fn nes_ra_destroy(client: *mut NativeClient);
    fn nes_ra_set_memory(client: *mut NativeClient, memory: *const u8, size: usize);
    fn nes_ra_login_password(
        client: *mut NativeClient,
        username: *const c_char,
        password: *const c_char,
    );
    fn nes_ra_login_token(client: *mut NativeClient, username: *const c_char, token: *const c_char);
    fn nes_ra_logout(client: *mut NativeClient);
    fn nes_ra_load_nes_game(
        client: *mut NativeClient,
        path: *const c_char,
        data: *const u8,
        size: usize,
    );
    fn nes_ra_unload_game(client: *mut NativeClient);
    fn nes_ra_do_frame(client: *mut NativeClient);
    fn nes_ra_idle(client: *mut NativeClient);
    fn nes_ra_reset(client: *mut NativeClient);
    fn nes_ra_take_request(client: *mut NativeClient, request: *mut NativeRequest) -> c_int;
    fn nes_ra_complete_request(
        client: *mut NativeClient,
        id: c_ulonglong,
        http_status: c_int,
        body: *const u8,
        body_size: usize,
    );
    fn nes_ra_pop_event(client: *mut NativeClient, event: *mut NativeEvent) -> c_int;
    fn nes_ra_get_user(client: *mut NativeClient, user: *mut NativeUser) -> c_int;
    fn nes_ra_get_game(client: *mut NativeClient, game: *mut NativeGame) -> c_int;
    fn nes_ra_get_achievements(
        client: *mut NativeClient,
        achievements: *mut NativeAchievement,
        capacity: usize,
    ) -> usize;
    fn nes_ra_is_game_loaded(client: *mut NativeClient) -> c_int;
    fn nes_ra_is_hardcore(client: *mut NativeClient) -> c_int;
}

#[derive(Debug, Clone)]
pub struct Request {
    pub id: u64,
    pub url: String,
    pub post_data: Option<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum EventKind {
    Login,
    GameLoad,
    Achievement,
    GameCompleted,
    ServerError,
    Disconnected,
    Reconnected,
    Reset,
    Leaderboard,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Event {
    pub kind: EventKind,
    pub result: i32,
    pub id: u32,
    pub points: u32,
    pub title: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    pub display_name: String,
    pub token: String,
    pub score: u32,
    pub score_softcore: u32,
}

#[derive(Debug, Clone)]
pub struct Game {
    pub id: u32,
    pub console_id: u32,
    pub title: String,
    pub hash: String,
}

#[derive(Debug, Clone)]
pub struct Achievement {
    pub id: u32,
    pub points: u32,
    pub unlocked: bool,
    pub bucket: AchievementBucket,
    pub measured_percent: f32,
    pub title: String,
    pub description: String,
    pub measured_progress: String,
    pub badge_url: String,
    pub badge_locked_url: String,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AchievementBucket {
    Unknown,
    Locked,
    Unlocked,
    Unsupported,
    Unofficial,
    RecentlyUnlocked,
    ActiveChallenge,
    AlmostThere,
    Unsynced,
}

impl From<u8> for AchievementBucket {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::Locked,
            2 => Self::Unlocked,
            3 => Self::Unsupported,
            4 => Self::Unofficial,
            5 => Self::RecentlyUnlocked,
            6 => Self::ActiveChallenge,
            7 => Self::AlmostThere,
            8 => Self::Unsynced,
            _ => Self::Unknown,
        }
    }
}

pub struct Client {
    native: NonNull<NativeClient>,
    _not_send_or_sync: PhantomData<*mut ()>,
}

impl Client {
    pub fn new() -> Result<Self, &'static str> {
        let native = NonNull::new(unsafe { nes_ra_create() })
            .ok_or("rcheevos could not allocate a client")?;
        Ok(Self {
            native,
            _not_send_or_sync: PhantomData,
        })
    }

    pub fn set_memory(&mut self, memory: &[u8]) {
        unsafe { nes_ra_set_memory(self.native.as_ptr(), memory.as_ptr(), memory.len()) };
    }

    pub fn login_password(&mut self, username: &str, password: &str) -> Result<(), String> {
        let username = c_string(username)?;
        let password = c_string(password)?;
        unsafe {
            nes_ra_login_password(self.native.as_ptr(), username.as_ptr(), password.as_ptr())
        };
        Ok(())
    }

    pub fn login_token(&mut self, username: &str, token: &str) -> Result<(), String> {
        let username = c_string(username)?;
        let token = c_string(token)?;
        unsafe { nes_ra_login_token(self.native.as_ptr(), username.as_ptr(), token.as_ptr()) };
        Ok(())
    }

    pub fn logout(&mut self) {
        unsafe { nes_ra_logout(self.native.as_ptr()) };
    }

    pub fn load_nes_game(&mut self, path: &str, data: &[u8]) -> Result<(), String> {
        let path = c_string(path)?;
        unsafe {
            nes_ra_load_nes_game(
                self.native.as_ptr(),
                path.as_ptr(),
                data.as_ptr(),
                data.len(),
            )
        };
        Ok(())
    }

    pub fn unload_game(&mut self) {
        unsafe { nes_ra_unload_game(self.native.as_ptr()) };
    }

    pub fn do_frame(&mut self) {
        unsafe { nes_ra_do_frame(self.native.as_ptr()) };
    }

    pub fn idle(&mut self) {
        unsafe { nes_ra_idle(self.native.as_ptr()) };
    }

    pub fn reset(&mut self) {
        unsafe { nes_ra_reset(self.native.as_ptr()) };
    }

    pub fn take_request(&mut self) -> Option<Request> {
        let mut native = NativeRequest {
            id: 0,
            has_post_data: 0,
            url: [0; URL_SIZE],
            post_data: [0; POST_SIZE],
        };
        if unsafe { nes_ra_take_request(self.native.as_ptr(), &mut native) } == 0 {
            return None;
        }
        Some(Request {
            id: native.id,
            url: char_buffer(&native.url),
            post_data: (native.has_post_data != 0).then(|| char_buffer(&native.post_data)),
        })
    }

    pub fn complete_request(&mut self, id: u64, http_status: i32, body: &[u8]) {
        unsafe {
            nes_ra_complete_request(
                self.native.as_ptr(),
                id,
                http_status,
                body.as_ptr(),
                body.len(),
            )
        };
    }

    pub fn pop_event(&mut self) -> Option<Event> {
        let mut native = NativeEvent {
            kind: 0,
            result: 0,
            id: 0,
            points: 0,
            title: [0; TITLE_SIZE],
            message: [0; MESSAGE_SIZE],
        };
        if unsafe { nes_ra_pop_event(self.native.as_ptr(), &mut native) } == 0 {
            return None;
        }
        Some(Event {
            kind: match native.kind {
                1 => EventKind::Login,
                2 => EventKind::GameLoad,
                3 => EventKind::Achievement,
                4 => EventKind::GameCompleted,
                5 => EventKind::ServerError,
                6 => EventKind::Disconnected,
                7 => EventKind::Reconnected,
                8 => EventKind::Reset,
                9 => EventKind::Leaderboard,
                _ => EventKind::Unknown,
            },
            result: native.result,
            id: native.id,
            points: native.points,
            title: char_buffer(&native.title),
            message: char_buffer(&native.message),
        })
    }

    pub fn user(&self) -> Option<User> {
        let mut native = NativeUser {
            score: 0,
            score_softcore: 0,
            username: [0; TITLE_SIZE],
            display_name: [0; TITLE_SIZE],
            token: [0; TOKEN_SIZE],
        };
        (unsafe { nes_ra_get_user(self.native.as_ptr(), &mut native) } != 0).then(|| User {
            username: char_buffer(&native.username),
            display_name: char_buffer(&native.display_name),
            token: char_buffer(&native.token),
            score: native.score,
            score_softcore: native.score_softcore,
        })
    }

    pub fn game(&self) -> Option<Game> {
        let mut native = NativeGame {
            id: 0,
            console_id: 0,
            title: [0; TITLE_SIZE],
            hash: [0; 40],
        };
        (unsafe { nes_ra_get_game(self.native.as_ptr(), &mut native) } != 0).then(|| Game {
            id: native.id,
            console_id: native.console_id,
            title: char_buffer(&native.title),
            hash: char_buffer(&native.hash),
        })
    }

    pub fn achievements(&mut self) -> Vec<Achievement> {
        let count =
            unsafe { nes_ra_get_achievements(self.native.as_ptr(), std::ptr::null_mut(), 0) };
        if count == 0 {
            return Vec::new();
        }
        let empty = NativeAchievement {
            id: 0,
            points: 0,
            unlocked: 0,
            bucket: 0,
            measured_percent: 0.0,
            title: [0; TITLE_SIZE],
            description: [0; MESSAGE_SIZE],
            measured_progress: [0; 32],
            badge_url: [0; IMAGE_URL_SIZE],
            badge_locked_url: [0; IMAGE_URL_SIZE],
        };
        let mut native = vec![empty; count];
        let written = unsafe {
            nes_ra_get_achievements(self.native.as_ptr(), native.as_mut_ptr(), native.len())
        }
        .min(native.len());
        native
            .into_iter()
            .take(written)
            .map(|item| Achievement {
                id: item.id,
                points: item.points,
                unlocked: item.unlocked != 0,
                bucket: item.bucket.into(),
                measured_percent: item.measured_percent,
                title: char_buffer(&item.title),
                description: char_buffer(&item.description),
                measured_progress: char_buffer(&item.measured_progress),
                badge_url: char_buffer(&item.badge_url),
                badge_locked_url: char_buffer(&item.badge_locked_url),
            })
            .collect()
    }

    pub fn is_game_loaded(&self) -> bool {
        unsafe { nes_ra_is_game_loaded(self.native.as_ptr()) != 0 }
    }

    pub fn is_hardcore(&self) -> bool {
        unsafe { nes_ra_is_hardcore(self.native.as_ptr()) != 0 }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        unsafe { nes_ra_destroy(self.native.as_ptr()) };
    }
}

fn c_string(value: &str) -> Result<CString, String> {
    CString::new(value).map_err(|_| "value contains an embedded NUL byte".to_owned())
}

fn char_buffer(buffer: &[c_char]) -> String {
    let pointer = buffer.as_ptr();
    unsafe { CStr::from_ptr(pointer) }
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::{Client, EventKind};

    #[test]
    fn client_starts_in_hardcore_without_a_loaded_game() {
        let client = Client::new().unwrap();
        assert!(client.is_hardcore());
        assert!(!client.is_game_loaded());
    }

    #[test]
    fn login_requests_can_be_polled_and_completed_by_the_host() {
        let mut client = Client::new().unwrap();
        client.login_token("test-user", "invalid-token").unwrap();
        let request = client.take_request().expect("login should request the API");
        assert!(request.url.starts_with("https://retroachievements.org/"));
        assert!(request.post_data.is_some());

        client.complete_request(request.id, 500, b"");
        let event = client
            .pop_event()
            .expect("failed login should produce an event");
        assert_eq!(event.kind, EventKind::Login);
        assert_ne!(event.result, 0);
    }
}
