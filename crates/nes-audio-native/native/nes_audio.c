#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "vendor/miniaudio.c"

typedef struct nes_audio_device {
    ma_device device;
    ma_pcm_rb ring;
    ma_uint32 target_frames;
    ma_uint32 capacity_frames;
    ma_bool32 callback_primed;
    ma_uint32 fade_in_remaining;
    float last_output;
    ma_atomic_uint32 underflows;
    ma_atomic_uint32 overflows;
    char device_name[MA_MAX_DEVICE_NAME_LENGTH + 32];
} nes_audio_device;

static void nes_audio_callback(
    ma_device* device,
    void* output,
    const void* input,
    ma_uint32 frame_count
) {
    nes_audio_device* audio = (nes_audio_device*)device->pUserData;
    float* destination = (float*)output;
    float* output_start = destination;
    ma_uint32 available;
    ma_uint32 read_frames;
    ma_uint32 remaining;

    (void)input;
    available = ma_pcm_rb_available_read(&audio->ring);

    if (!audio->callback_primed) {
        if (available < audio->target_frames) {
            memset(destination, 0, sizeof(float) * frame_count * 2);
            return;
        }
        audio->callback_primed = MA_TRUE;
        audio->fade_in_remaining = 64;
        audio->last_output = 0.0f;
    }

    read_frames = available < frame_count ? available : frame_count;
    remaining = read_frames;
    while (remaining > 0) {
        void* source = NULL;
        ma_uint32 chunk = remaining;
        if (ma_pcm_rb_acquire_read(&audio->ring, &chunk, &source) != MA_SUCCESS || chunk == 0) {
            read_frames -= remaining;
            break;
        }
        memcpy(destination, source, sizeof(float) * chunk * 2);
        ma_pcm_rb_commit_read(&audio->ring, chunk);
        destination += chunk * 2;
        remaining -= chunk;
    }

    {
        ma_uint32 i;
        for (i = 0; i < read_frames; ++i) {
            if (audio->fade_in_remaining > 0) {
                const float gain = (float)(65 - audio->fade_in_remaining) / 64.0f;
                output_start[i * 2 + 0] *= gain;
                output_start[i * 2 + 1] *= gain;
                audio->fade_in_remaining -= 1;
            }
            audio->last_output = output_start[i * 2];
        }
    }

    if (read_frames < frame_count) {
        ma_uint32 i;
        const ma_uint32 missing = frame_count - read_frames;
        const ma_uint32 fade_frames = missing < 32 ? missing : 32;
        float* tail = output_start + read_frames * 2;
        for (i = 0; i < fade_frames; ++i) {
            const float gain = 1.0f - ((float)(i + 1) / (float)fade_frames);
            tail[i * 2 + 0] = audio->last_output * gain;
            tail[i * 2 + 1] = audio->last_output * gain;
        }
        if (missing > fade_frames) {
            memset(tail + fade_frames * 2, 0, sizeof(float) * (missing - fade_frames) * 2);
        }
        audio->callback_primed = MA_FALSE;
        audio->fade_in_remaining = 0;
        audio->last_output = 0.0f;
        ma_atomic_uint32_fetch_add(&audio->underflows, 1);
    }
}

nes_audio_device* nes_audio_create(
    uint32_t sample_rate,
    uint32_t target_frames,
    uint32_t capacity_frames,
    char* error,
    size_t error_capacity
) {
    nes_audio_device* audio;
    ma_device_config config;
    ma_result result;

    if (sample_rate == 0 || target_frames == 0 || capacity_frames < target_frames * 2) {
        snprintf(error, error_capacity, "invalid native audio buffer configuration");
        return NULL;
    }

    audio = (nes_audio_device*)calloc(1, sizeof(*audio));
    if (audio == NULL) {
        snprintf(error, error_capacity, "could not allocate native audio device");
        return NULL;
    }

    audio->target_frames = target_frames;
    audio->capacity_frames = capacity_frames;
    ma_atomic_uint32_set(&audio->underflows, 0);
    ma_atomic_uint32_set(&audio->overflows, 0);

    result = ma_pcm_rb_init(ma_format_f32, 2, capacity_frames, NULL, NULL, &audio->ring);
    if (result != MA_SUCCESS) {
        snprintf(error, error_capacity, "could not initialize native PCM ring: %s", ma_result_description(result));
        free(audio);
        return NULL;
    }

    config = ma_device_config_init(ma_device_type_playback);
    config.playback.format = ma_format_f32;
    config.playback.channels = 2;
    config.sampleRate = sample_rate;
    config.periodSizeInMilliseconds = 10;
    config.periods = 3;
    config.performanceProfile = ma_performance_profile_low_latency;
    config.dataCallback = nes_audio_callback;
    config.pUserData = audio;
    config.wasapi.noAutoConvertSRC = MA_TRUE;

    result = ma_device_init(NULL, &config, &audio->device);
    if (result != MA_SUCCESS) {
        snprintf(error, error_capacity, "could not initialize miniaudio device: %s", ma_result_description(result));
        ma_pcm_rb_uninit(&audio->ring);
        free(audio);
        return NULL;
    }

    snprintf(
        audio->device_name,
        sizeof(audio->device_name),
        "miniaudio/WASAPI - %s",
        audio->device.playback.name
    );
    return audio;
}

void nes_audio_destroy(nes_audio_device* audio) {
    if (audio == NULL) {
        return;
    }
    ma_device_uninit(&audio->device);
    ma_pcm_rb_uninit(&audio->ring);
    free(audio);
}

uint32_t nes_audio_push(nes_audio_device* audio, const float* mono_samples, uint32_t frame_count) {
    ma_uint32 writable;
    ma_uint32 total;

    if (audio == NULL || mono_samples == NULL || frame_count == 0) {
        return 0;
    }

    writable = ma_pcm_rb_available_write(&audio->ring);
    total = frame_count < writable ? frame_count : writable;
    if (total < frame_count) {
        ma_atomic_uint32_fetch_add(&audio->overflows, 1);
    }

    {
        ma_uint32 written = 0;
        while (written < total) {
            void* destination_raw = NULL;
            float* destination;
            ma_uint32 chunk = total - written;
            ma_uint32 i;
            if (ma_pcm_rb_acquire_write(&audio->ring, &chunk, &destination_raw) != MA_SUCCESS || chunk == 0) {
                break;
            }
            destination = (float*)destination_raw;
            for (i = 0; i < chunk; ++i) {
                const float sample = mono_samples[written + i];
                destination[i * 2 + 0] = sample;
                destination[i * 2 + 1] = sample;
            }
            ma_pcm_rb_commit_write(&audio->ring, chunk);
            written += chunk;
        }
        total = written;
    }

    if (ma_device_get_state(&audio->device) == ma_device_state_stopped &&
        ma_pcm_rb_available_read(&audio->ring) >= audio->target_frames) {
        audio->callback_primed = MA_TRUE;
        audio->fade_in_remaining = 64;
        audio->last_output = 0.0f;
        ma_device_start(&audio->device);
    }

    return total;
}

void nes_audio_clear(nes_audio_device* audio) {
    if (audio == NULL) {
        return;
    }
    if (ma_device_get_state(&audio->device) == ma_device_state_started) {
        ma_device_stop(&audio->device);
    }
    audio->callback_primed = MA_FALSE;
    audio->fade_in_remaining = 0;
    audio->last_output = 0.0f;
    ma_pcm_rb_reset(&audio->ring);
}

uint32_t nes_audio_queued(const nes_audio_device* audio) {
    return audio == NULL ? 0 : ma_pcm_rb_available_read((ma_pcm_rb*)&audio->ring);
}

uint32_t nes_audio_underflows(const nes_audio_device* audio) {
    return audio == NULL ? 0 : ma_atomic_uint32_get((ma_atomic_uint32*)&audio->underflows);
}

uint32_t nes_audio_overflows(const nes_audio_device* audio) {
    return audio == NULL ? 0 : ma_atomic_uint32_get((ma_atomic_uint32*)&audio->overflows);
}

uint32_t nes_audio_device_rate(const nes_audio_device* audio) {
    return audio == NULL ? 0 : audio->device.playback.internalSampleRate;
}

const char* nes_audio_device_name(const nes_audio_device* audio) {
    return audio == NULL ? "No native audio device" : audio->device_name;
}
