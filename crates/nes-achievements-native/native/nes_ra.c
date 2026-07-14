#include "nes_ra.h"

#include "rcheevos/include/rc_api_request.h"
#include "rcheevos/include/rc_client.h"
#include "rcheevos/include/rc_consoles.h"

#include <stdlib.h>
#include <string.h>

typedef struct nes_ra_request_node {
    uint64_t id;
    int dispatched;
    char* url;
    char* post_data;
    rc_client_server_callback_t callback;
    void* callback_data;
    struct nes_ra_request_node* next;
} nes_ra_request_node;

typedef struct nes_ra_callback_context {
    struct nes_ra_client* owner;
    int event_type;
} nes_ra_callback_context;

struct nes_ra_client {
    rc_client_t* client;
    uint8_t memory[0x10000];
    uint64_t next_request_id;
    nes_ra_request_node* requests;
    nes_ra_event_out events[64];
    size_t event_read;
    size_t event_count;
    uint8_t* load_data;
    char* load_path;
};

static char* nes_ra_strdup(const char* value) {
    size_t size;
    char* copy;
    if (!value) return NULL;
    size = strlen(value) + 1;
    copy = (char*)malloc(size);
    if (copy) memcpy(copy, value, size);
    return copy;
}

static void nes_ra_copy(char* target, size_t target_size, const char* source) {
    if (!target_size) return;
    if (!source) source = "";
    strncpy(target, source, target_size - 1);
    target[target_size - 1] = '\0';
}

static void nes_ra_push_event(nes_ra_client* owner, int type, int result, uint32_t id,
                              uint32_t points, const char* title, const char* message) {
    size_t index;
    nes_ra_event_out* event;
    if (owner->event_count == 64) {
        owner->event_read = (owner->event_read + 1) % 64;
        owner->event_count--;
    }
    index = (owner->event_read + owner->event_count) % 64;
    event = &owner->events[index];
    memset(event, 0, sizeof(*event));
    event->type = type;
    event->result = result;
    event->id = id;
    event->points = points;
    nes_ra_copy(event->title, sizeof(event->title), title);
    nes_ra_copy(event->message, sizeof(event->message), message);
    owner->event_count++;
}

static uint32_t RC_CCONV nes_ra_read_memory(uint32_t address, uint8_t* buffer,
                                            uint32_t num_bytes, rc_client_t* client) {
    nes_ra_client* owner = (nes_ra_client*)rc_client_get_userdata(client);
    if (!owner || address >= sizeof(owner->memory)) return 0;
    if (num_bytes > sizeof(owner->memory) - address)
        num_bytes = (uint32_t)(sizeof(owner->memory) - address);
    memcpy(buffer, owner->memory + address, num_bytes);
    return num_bytes;
}

static void RC_CCONV nes_ra_server_call(const rc_api_request_t* request,
                                        rc_client_server_callback_t callback,
                                        void* callback_data, rc_client_t* client) {
    nes_ra_client* owner = (nes_ra_client*)rc_client_get_userdata(client);
    nes_ra_request_node* node;
    nes_ra_request_node** tail;
    rc_api_server_response_t response;
    if (!owner) return;
    node = (nes_ra_request_node*)calloc(1, sizeof(*node));
    if (!node) {
        memset(&response, 0, sizeof(response));
        response.http_status_code = RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR;
        callback(&response, callback_data);
        return;
    }
    node->id = ++owner->next_request_id;
    node->url = nes_ra_strdup(request->url);
    node->post_data = nes_ra_strdup(request->post_data);
    if (!node->url || (request->post_data && !node->post_data)) {
        free(node->url);
        free(node->post_data);
        free(node);
        memset(&response, 0, sizeof(response));
        response.http_status_code = RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR;
        callback(&response, callback_data);
        return;
    }
    node->callback = callback;
    node->callback_data = callback_data;
    tail = &owner->requests;
    while (*tail) tail = &(*tail)->next;
    *tail = node;
}

static void RC_CCONV nes_ra_async_callback(int result, const char* error_message,
                                           rc_client_t* client, void* userdata) {
    nes_ra_callback_context* context = (nes_ra_callback_context*)userdata;
    nes_ra_client* owner = context ? context->owner : (nes_ra_client*)rc_client_get_userdata(client);
    if (owner)
        nes_ra_push_event(owner, context ? context->event_type : 0, result, 0, 0,
                          result == RC_OK ? "Success" : "RetroAchievements error",
                          error_message);
    if (owner && context && context->event_type == NES_RA_EVENT_GAME_LOAD) {
        free(owner->load_data);
        free(owner->load_path);
        owner->load_data = NULL;
        owner->load_path = NULL;
    }
    free(context);
}

static void RC_CCONV nes_ra_event_handler(const rc_client_event_t* event, rc_client_t* client) {
    nes_ra_client* owner = (nes_ra_client*)rc_client_get_userdata(client);
    if (!owner || !event) return;
    switch (event->type) {
        case RC_CLIENT_EVENT_ACHIEVEMENT_TRIGGERED:
            nes_ra_push_event(owner, NES_RA_EVENT_ACHIEVEMENT, RC_OK,
                              event->achievement->id, event->achievement->points,
                              event->achievement->title, event->achievement->description);
            break;
        case RC_CLIENT_EVENT_GAME_COMPLETED:
            nes_ra_push_event(owner, NES_RA_EVENT_GAME_COMPLETED, RC_OK, 0, 0,
                              "Game mastered", "All core achievements unlocked");
            break;
        case RC_CLIENT_EVENT_SERVER_ERROR:
            nes_ra_push_event(owner, NES_RA_EVENT_SERVER_ERROR, event->server_error->result,
                              event->server_error->related_id, 0, event->server_error->api,
                              event->server_error->error_message);
            break;
        case RC_CLIENT_EVENT_DISCONNECTED:
            nes_ra_push_event(owner, NES_RA_EVENT_DISCONNECTED, 0, 0, 0,
                              "Disconnected", "Unlock submissions will be retried");
            break;
        case RC_CLIENT_EVENT_RECONNECTED:
            nes_ra_push_event(owner, NES_RA_EVENT_RECONNECTED, 0, 0, 0,
                              "Reconnected", "Pending unlocks submitted");
            break;
        case RC_CLIENT_EVENT_RESET:
            nes_ra_push_event(owner, NES_RA_EVENT_RESET, 0, 0, 0,
                              "Reset required", "Hardcore mode requested a reset");
            break;
        case RC_CLIENT_EVENT_LEADERBOARD_STARTED:
            nes_ra_push_event(owner, NES_RA_EVENT_LEADERBOARD, 0, event->leaderboard->id, 0,
                              event->leaderboard->title, "Leaderboard attempt started");
            break;
        case RC_CLIENT_EVENT_LEADERBOARD_FAILED:
            nes_ra_push_event(owner, NES_RA_EVENT_LEADERBOARD, 0, event->leaderboard->id, 0,
                              event->leaderboard->title, "Leaderboard attempt failed");
            break;
        case RC_CLIENT_EVENT_LEADERBOARD_SUBMITTED:
            nes_ra_push_event(owner, NES_RA_EVENT_LEADERBOARD, 0, event->leaderboard->id, 0,
                              event->leaderboard->title, event->leaderboard->tracker_value);
            break;
        default:
            break;
    }
}

nes_ra_client* nes_ra_create(void) {
    nes_ra_client* owner = (nes_ra_client*)calloc(1, sizeof(*owner));
    if (!owner) return NULL;
    owner->client = rc_client_create(nes_ra_read_memory, nes_ra_server_call);
    if (!owner->client) {
        free(owner);
        return NULL;
    }
    rc_client_set_userdata(owner->client, owner);
    rc_client_set_event_handler(owner->client, nes_ra_event_handler);
    rc_client_set_hardcore_enabled(owner->client, 1);
    rc_client_set_unofficial_enabled(owner->client, 0);
    rc_client_set_encore_mode_enabled(owner->client, 0);
    return owner;
}

void nes_ra_destroy(nes_ra_client* owner) {
    nes_ra_request_node* node;
    if (!owner) return;
    rc_client_destroy(owner->client);
    while ((node = owner->requests) != NULL) {
        owner->requests = node->next;
        free(node->url);
        free(node->post_data);
        free(node);
    }
    free(owner->load_data);
    free(owner->load_path);
    free(owner);
}

void nes_ra_set_memory(nes_ra_client* owner, const uint8_t* memory, size_t size) {
    if (!owner || !memory) return;
    if (size > sizeof(owner->memory)) size = sizeof(owner->memory);
    memcpy(owner->memory, memory, size);
    if (size < sizeof(owner->memory)) memset(owner->memory + size, 0, sizeof(owner->memory) - size);
}

static nes_ra_callback_context* nes_ra_context(nes_ra_client* owner, int event_type) {
    nes_ra_callback_context* context = (nes_ra_callback_context*)malloc(sizeof(*context));
    if (context) {
        context->owner = owner;
        context->event_type = event_type;
    }
    return context;
}

void nes_ra_login_password(nes_ra_client* owner, const char* username, const char* password) {
    nes_ra_callback_context* context;
    if (!owner) return;
    context = nes_ra_context(owner, NES_RA_EVENT_LOGIN);
    if (!context) {
        nes_ra_push_event(owner, NES_RA_EVENT_LOGIN, -1, 0, 0,
                          "RetroAchievements error", "Could not allocate login request");
        return;
    }
    rc_client_begin_login_with_password(owner->client, username, password, nes_ra_async_callback, context);
}

void nes_ra_login_token(nes_ra_client* owner, const char* username, const char* token) {
    nes_ra_callback_context* context;
    if (!owner) return;
    context = nes_ra_context(owner, NES_RA_EVENT_LOGIN);
    if (!context) {
        nes_ra_push_event(owner, NES_RA_EVENT_LOGIN, -1, 0, 0,
                          "RetroAchievements error", "Could not allocate login request");
        return;
    }
    rc_client_begin_login_with_token(owner->client, username, token, nes_ra_async_callback, context);
}

void nes_ra_logout(nes_ra_client* owner) {
    if (owner) rc_client_logout(owner->client);
}

void nes_ra_load_nes_game(nes_ra_client* owner, const char* path, const uint8_t* data, size_t size) {
    nes_ra_callback_context* context;
    if (!owner || !data || !size) return;
    rc_client_unload_game(owner->client);
    free(owner->load_data);
    free(owner->load_path);
    owner->load_data = (uint8_t*)malloc(size);
    owner->load_path = nes_ra_strdup(path);
    if (!owner->load_data || !owner->load_path) {
        free(owner->load_data);
        free(owner->load_path);
        owner->load_data = NULL;
        owner->load_path = NULL;
        nes_ra_push_event(owner, NES_RA_EVENT_GAME_LOAD, -1, 0, 0,
                          "RetroAchievements error", "Could not buffer ROM for hashing");
        return;
    }
    memcpy(owner->load_data, data, size);
    context = nes_ra_context(owner, NES_RA_EVENT_GAME_LOAD);
    if (!context) {
        free(owner->load_data);
        free(owner->load_path);
        owner->load_data = NULL;
        owner->load_path = NULL;
        nes_ra_push_event(owner, NES_RA_EVENT_GAME_LOAD, -1, 0, 0,
                          "RetroAchievements error", "Could not allocate game request");
        return;
    }
    rc_client_begin_identify_and_load_game(owner->client, RC_CONSOLE_NINTENDO,
        owner->load_path, owner->load_data, size, nes_ra_async_callback, context);
}

void nes_ra_unload_game(nes_ra_client* owner) {
    if (owner) rc_client_unload_game(owner->client);
}
void nes_ra_do_frame(nes_ra_client* owner) { if (owner) rc_client_do_frame(owner->client); }
void nes_ra_idle(nes_ra_client* owner) { if (owner) rc_client_idle(owner->client); }
void nes_ra_reset(nes_ra_client* owner) { if (owner) rc_client_reset(owner->client); }

int nes_ra_take_request(nes_ra_client* owner, nes_ra_request_out* output) {
    nes_ra_request_node* node;
    if (!owner || !output) return 0;
    for (node = owner->requests; node; node = node->next) {
        if (!node->dispatched) {
            memset(output, 0, sizeof(*output));
            output->id = node->id;
            output->has_post_data = node->post_data != NULL;
            nes_ra_copy(output->url, sizeof(output->url), node->url);
            nes_ra_copy(output->post_data, sizeof(output->post_data), node->post_data);
            node->dispatched = 1;
            return 1;
        }
    }
    return 0;
}

void nes_ra_complete_request(nes_ra_client* owner, uint64_t id, int http_status,
                             const uint8_t* body, size_t body_size) {
    nes_ra_request_node** link;
    nes_ra_request_node* node;
    rc_api_server_response_t response;
    char* terminated_body;
    if (!owner) return;
    link = &owner->requests;
    while (*link && (*link)->id != id) link = &(*link)->next;
    if (!*link) return;
    node = *link;
    *link = node->next;
    terminated_body = (char*)malloc(body_size + 1);
    if (terminated_body) {
        if (body && body_size) memcpy(terminated_body, body, body_size);
        terminated_body[body_size] = '\0';
    }
    memset(&response, 0, sizeof(response));
    response.body = terminated_body;
    response.body_length = terminated_body ? body_size : 0;
    response.http_status_code = terminated_body
        ? (http_status ? http_status : RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR)
        : RC_API_SERVER_RESPONSE_RETRYABLE_CLIENT_ERROR;
    node->callback(&response, node->callback_data);
    free(terminated_body);
    free(node->url);
    free(node->post_data);
    free(node);
}

int nes_ra_pop_event(nes_ra_client* owner, nes_ra_event_out* event) {
    if (!owner || !event || !owner->event_count) return 0;
    *event = owner->events[owner->event_read];
    owner->event_read = (owner->event_read + 1) % 64;
    owner->event_count--;
    return 1;
}

int nes_ra_get_user(nes_ra_client* owner, nes_ra_user_out* output) {
    const rc_client_user_t* user;
    if (!owner || !output || !(user = rc_client_get_user_info(owner->client))) return 0;
    memset(output, 0, sizeof(*output));
    output->score = user->score;
    output->score_softcore = user->score_softcore;
    nes_ra_copy(output->username, sizeof(output->username), user->username);
    nes_ra_copy(output->display_name, sizeof(output->display_name), user->display_name);
    nes_ra_copy(output->token, sizeof(output->token), user->token);
    return 1;
}

int nes_ra_get_game(nes_ra_client* owner, nes_ra_game_out* output) {
    const rc_client_game_t* game;
    if (!owner || !output || !(game = rc_client_get_game_info(owner->client))) return 0;
    memset(output, 0, sizeof(*output));
    output->id = game->id;
    output->console_id = game->console_id;
    nes_ra_copy(output->title, sizeof(output->title), game->title);
    nes_ra_copy(output->hash, sizeof(output->hash), game->hash);
    return 1;
}

size_t nes_ra_get_achievements(nes_ra_client* owner, nes_ra_achievement_out* output, size_t capacity) {
    rc_client_achievement_list_t* list;
    size_t count = 0;
    uint32_t bucket_index;
    if (!owner) return 0;
    list = rc_client_create_achievement_list(owner->client, RC_CLIENT_ACHIEVEMENT_CATEGORY_CORE,
                                             RC_CLIENT_ACHIEVEMENT_LIST_GROUPING_PROGRESS);
    if (!list) return 0;
    for (bucket_index = 0; bucket_index < list->num_buckets; ++bucket_index) {
        const rc_client_achievement_bucket_t* bucket = &list->buckets[bucket_index];
        uint32_t achievement_index;
        for (achievement_index = 0; achievement_index < bucket->num_achievements; ++achievement_index) {
            const rc_client_achievement_t* achievement = bucket->achievements[achievement_index];
            if (output && count < capacity) {
                nes_ra_achievement_out* item = &output[count];
                memset(item, 0, sizeof(*item));
                item->id = achievement->id;
                item->points = achievement->points;
                item->unlocked = achievement->unlocked;
                item->bucket = achievement->bucket;
                item->measured_percent = achievement->measured_percent;
                nes_ra_copy(item->title, sizeof(item->title), achievement->title);
                nes_ra_copy(item->description, sizeof(item->description), achievement->description);
                nes_ra_copy(item->measured_progress, sizeof(item->measured_progress), achievement->measured_progress);
                rc_client_achievement_get_image_url(achievement,
                    RC_CLIENT_ACHIEVEMENT_STATE_UNLOCKED, item->badge_url,
                    sizeof(item->badge_url));
                rc_client_achievement_get_image_url(achievement,
                    RC_CLIENT_ACHIEVEMENT_STATE_ACTIVE, item->badge_locked_url,
                    sizeof(item->badge_locked_url));
            }
            count++;
        }
    }
    rc_client_destroy_achievement_list(list);
    return count;
}

int nes_ra_is_game_loaded(nes_ra_client* owner) {
    return owner ? rc_client_is_game_loaded(owner->client) : 0;
}
int nes_ra_is_hardcore(nes_ra_client* owner) {
    return owner ? rc_client_get_hardcore_enabled(owner->client) : 0;
}
