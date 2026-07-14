#ifndef NES_RA_H
#define NES_RA_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NES_RA_URL_SIZE 2048
#define NES_RA_POST_SIZE 8192
#define NES_RA_TITLE_SIZE 160
#define NES_RA_MESSAGE_SIZE 320
#define NES_RA_TOKEN_SIZE 128
#define NES_RA_IMAGE_URL_SIZE 512

typedef struct nes_ra_client nes_ra_client;

typedef struct nes_ra_request_out {
    uint64_t id;
    int has_post_data;
    char url[NES_RA_URL_SIZE];
    char post_data[NES_RA_POST_SIZE];
} nes_ra_request_out;

typedef struct nes_ra_event_out {
    int type;
    int result;
    uint32_t id;
    uint32_t points;
    char title[NES_RA_TITLE_SIZE];
    char message[NES_RA_MESSAGE_SIZE];
} nes_ra_event_out;

typedef struct nes_ra_user_out {
    uint32_t score;
    uint32_t score_softcore;
    char username[NES_RA_TITLE_SIZE];
    char display_name[NES_RA_TITLE_SIZE];
    char token[NES_RA_TOKEN_SIZE];
} nes_ra_user_out;

typedef struct nes_ra_game_out {
    uint32_t id;
    uint32_t console_id;
    char title[NES_RA_TITLE_SIZE];
    char hash[40];
} nes_ra_game_out;

typedef struct nes_ra_achievement_out {
    uint32_t id;
    uint32_t points;
    uint8_t unlocked;
    uint8_t bucket;
    float measured_percent;
    char title[NES_RA_TITLE_SIZE];
    char description[NES_RA_MESSAGE_SIZE];
    char measured_progress[32];
    char badge_url[NES_RA_IMAGE_URL_SIZE];
    char badge_locked_url[NES_RA_IMAGE_URL_SIZE];
} nes_ra_achievement_out;

enum {
    NES_RA_EVENT_LOGIN = 1,
    NES_RA_EVENT_GAME_LOAD = 2,
    NES_RA_EVENT_ACHIEVEMENT = 3,
    NES_RA_EVENT_GAME_COMPLETED = 4,
    NES_RA_EVENT_SERVER_ERROR = 5,
    NES_RA_EVENT_DISCONNECTED = 6,
    NES_RA_EVENT_RECONNECTED = 7,
    NES_RA_EVENT_RESET = 8,
    NES_RA_EVENT_LEADERBOARD = 9
};

nes_ra_client* nes_ra_create(void);
void nes_ra_destroy(nes_ra_client* client);
void nes_ra_set_memory(nes_ra_client* client, const uint8_t* memory, size_t size);
void nes_ra_login_password(nes_ra_client* client, const char* username, const char* password);
void nes_ra_login_token(nes_ra_client* client, const char* username, const char* token);
void nes_ra_logout(nes_ra_client* client);
void nes_ra_load_nes_game(nes_ra_client* client, const char* path, const uint8_t* data, size_t size);
void nes_ra_unload_game(nes_ra_client* client);
void nes_ra_do_frame(nes_ra_client* client);
void nes_ra_idle(nes_ra_client* client);
void nes_ra_reset(nes_ra_client* client);
int nes_ra_take_request(nes_ra_client* client, nes_ra_request_out* request);
void nes_ra_complete_request(nes_ra_client* client, uint64_t id, int http_status, const uint8_t* body, size_t body_size);
int nes_ra_pop_event(nes_ra_client* client, nes_ra_event_out* event);
int nes_ra_get_user(nes_ra_client* client, nes_ra_user_out* user);
int nes_ra_get_game(nes_ra_client* client, nes_ra_game_out* game);
size_t nes_ra_get_achievements(nes_ra_client* client, nes_ra_achievement_out* achievements, size_t capacity);
int nes_ra_is_game_loaded(nes_ra_client* client);
int nes_ra_is_hardcore(nes_ra_client* client);
int nes_ra_hash_nes_game(const uint8_t* data, size_t size, char hash[33]);

#ifdef __cplusplus
}
#endif
#endif
