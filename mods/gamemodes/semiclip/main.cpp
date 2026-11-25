// cs16-rs Teammate Semiclip (`semiclip`) — C++20 -> WASM
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Features:
// - True engine-level teammate pass-through via `PlayersShouldCollide` (Pre -> Supercede with 0)
// - Smooth distance-based `RenderMode::TransAlpha` fade when overlapping teammates (< 96 units)

#include "cs16_mod.hpp"

CS16_PLUGIN_INFO("semiclip", "Teammate Semiclip & TransAlpha Fade", "1.0.0")

namespace {

constexpr float FADE_RADIUS_SQ = 96.0f * 96.0f;
static uint8_t g_last_alpha[32] = {
    255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255
};

} // namespace

CS16_ON_HOOK(cs16::HookId::PlayersShouldCollide, cs16::HookPhase::Pre, on_players_collide) {
    cs16::Player a(cs16_hook_arg_i32(0));
    cs16::Player b(cs16_hook_arg_i32(1));
    if (a.slot == b.slot) {
        return cs16::HookAction::Continue;
    }
    auto ta = a.team();
    auto tb = b.team();
    if ((ta == cs16::Team::Terrorist || ta == cs16::Team::Ct) && ta == tb) {
        cs16_hook_set_override_i32(0);
        return cs16::HookAction::Supercede;
    }
    return cs16::HookAction::Continue;
}

CS16_ON_HOOK(cs16::HookId::PlayerPostThink, cs16::HookPhase::Post, on_post_think) {
    int32_t slot = cs16_hook_arg_i32(0);
    if (slot < 0 || slot >= 32) {
        return cs16::HookAction::Continue;
    }
    cs16::Player p(slot);
    if (!p.is_alive()) {
        return cs16::HookAction::Continue;
    }
    auto team = p.team();
    cs16::Vec3 pos;
    if (!p.origin(pos)) {
        return cs16::HookAction::Continue;
    }

    float min_sq = FADE_RADIUS_SQ;
    int32_t max_c = cs16::server::max_clients();
    for (int32_t other = 0; other < max_c; ++other) {
        if (other == slot) continue;
        cs16::Player op(other);
        if (!op.is_alive() || op.team() != team) continue;
        cs16::Vec3 opos;
        if (!op.origin(opos)) continue;
        float d2 = pos.dist_sq(opos);
        if (d2 < min_sq) {
            min_sq = d2;
        }
    }

    uint8_t target_alpha = 255;
    if (min_sq < FADE_RADIUS_SQ) {
        float ratio = min_sq / FADE_RADIUS_SQ;
        int32_t computed = 95 + static_cast<int32_t>(ratio * 160.0f);
        if (computed < 95) computed = 95;
        if (computed > 255) computed = 255;
        target_alpha = static_cast<uint8_t>(computed);
    }

    if (g_last_alpha[slot] != target_alpha) {
        g_last_alpha[slot] = target_alpha;
        if (target_alpha < 255) {
            p.set_rendering(CS16_RENDERFX_NONE, 255, 255, 255, CS16_RENDER_TRANSALPHA, target_alpha);
        } else {
            p.set_rendering(CS16_RENDERFX_NONE, 255, 255, 255, CS16_RENDER_NORMAL, 255);
        }
    }
    return cs16::HookAction::Continue;
}

CS16_EXPORT_PLUGIN()
