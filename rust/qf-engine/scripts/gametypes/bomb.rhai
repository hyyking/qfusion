// Bomb and Defuse gametype (port of source/server/game/progs/gametypes/bomb).
//
// This script owns the rules: match and round lifecycle, roles, bomb carrier, planting,
// arming, defusing, explosions, scoring and the HUD data. It is not tied to a map: it
// finds spawns and bomb sites by entity classname through the generic host API
// (`g.entities()`, `g.trace()`, `g.los()`, `g.spawn()`, ...). Teams are 0 (alpha) and
// 1 (beta). Times are milliseconds unless a config value says seconds.

// Legacy cvar defaults: g_bomb_roundtime, g_bomb_bombtimer, g_bomb_armtime,
// g_bomb_defusetime, g_bomb_carriers, g_bomb_spawnprotection, g_scorelimit,
// g_countdown_time, g_timelimit.
fn config() {
    #{
        round_time: 60,
        bomb_timer: 30,
        arm_time: 4,
        defuse_time: 7,
        carriers: true,
        spawn_protection: 3,
        score_limit: 16,
        countdown_time: 5,
        time_limit_minutes: 0,
    }
}

fn ms(seconds) {
    (seconds * 1000.0).round().to_int()
}

// True when attackers and defenders trade places before the round about to start.
// `round_count` is the number of rounds already started. Mirrors round.as
// roundNewState(ROUNDSTATE_PRE), including its quirk of swapping every round once
// round_count >= 2 * (score_limit - 1).
fn swap_roles(round_count, score_limit) {
    if score_limit == 0 {
        return true;
    }
    let limit = score_limit - 1;
    round_count == limit || round_count >= limit * 2
}

fn new_bomb() {
    #{
        state: "idle",
        origin: [0.0, 0.0, 0.0],
        velocity: [0.0, 0.0, 0.0],
        carrier: -1,
        site: -1,
        since: 0,
        explode_at: 0,
        arm: 0,
        defuse: 0,
        dropper: -1,
        retake_at: 0,
        hurry: false,
    }
}

fn overlaps(a_mins, a_maxs, b_mins, b_maxs) {
    for axis in 0..3 {
        if a_mins[axis] > b_maxs[axis] || a_maxs[axis] < b_mins[axis] {
            return false;
        }
    }
    true
}

fn player_bounds(origin) {
    #{ mins: vadd(origin, [-16.0, -16.0, -24.0]), maxs: vadd(origin, [16.0, 16.0, 40.0]) }
}

fn bomb_bounds(origin) {
    #{ mins: vadd(origin, [-16.0, -16.0, -8.0]), maxs: vadd(origin, [16.0, 16.0, 48.0]) }
}

fn spawn_point(e) {
    let yaw = 0.0;
    if e.angles != () {
        yaw = e.angles[1];
    } else if e.angle != () {
        yaw = to_num(e.angle, 0.0);
    }
    #{ origin: e.origin, yaw: yaw }
}

fn ammo_for(weapon) {
    if weapon == "rl" || weapon == "eb" || weapon == "rg" { 15 }
    else if weapon == "lg" { 180 }
    else if weapon == "pg" { 140 }
    else if weapon == "mg" { 150 }
    else if weapon == "gl" { 10 }
    else if weapon == "gb" { 1 }
    else { 0 }
}

// ---------------------------------------------------------------- setup

fn init(g) {
    let cfg = config();
    this.cfg = #{
        round: ms(cfg.round_time),
        bomb: ms(cfg.bomb_timer),
        arm: ms(cfg.arm_time),
        defuse: ms(cfg.defuse_time),
        carriers: cfg.carriers,
        protect: cfg.spawn_protection,
        score_limit: cfg.score_limit,
        countdown: ms(cfg.countdown_time),
        time_limit: ms(cfg.time_limit_minutes * 60),
    };
    this.started = false;
    this.match_state = "countdown";
    this.state_since = 0;
    this.match_started = 0;
    this.round_state = "pre";
    this.round_started = 0;
    this.round_count = 0;
    this.attacker = 0;
    this.scores = [0, 0];
    this.team_at_start = [0, 0];
    this.bomb = new_bomb();
    this.pending = [];
    this.winner = -1;
    this.loads = #{};
    this.atk_spawns = [];
    this.def_spawns = [];
    this.sites = [];
    this.scan_map(g);
    if this.atk_spawns.len() == 0 || this.def_spawns.len() == 0 || this.sites.len() == 0 {
        throw "map needs team_CTF_alphaspawn, team_CTF_betaspawn and misc_capture_area_indicator";
    }
}

// Legacy roles pick spawns by role, not team colour: attackers use the beta spawns.
fn scan_map(g) {
    let entities = g.entities();
    let letters = ["A", "B"];
    for e in entities {
        if e.classname == () || e.origin == () {
            continue;
        }
        let class = e.classname.to_lower();
        if class == "team_ctf_betaspawn" {
            this.atk_spawns.push(spawn_point(e));
        } else if class == "team_ctf_alphaspawn" {
            this.def_spawns.push(spawn_point(e));
        }
    }
    for e in entities {
        if e.classname == () || e.origin == () || e.classname.to_lower() != "misc_capture_area_indicator" {
            continue;
        }
        if this.sites.len() >= 2 {
            break;
        }
        let origin = e.origin;
        let flags = 0;
        if e.spawnflags != () {
            flags = to_num(e.spawnflags, 0.0).to_int();
        }
        // Spawnflag 1 keeps the indicator where the mapper put it.
        if (flags & 1) == 0 {
            let t = g.trace(vadd(origin, [0.0, 0.0, 16.0]), vadd(origin, [0.0, 0.0, -512.0]), [-16.0, -16.0, -24.0], [16.0, 16.0, 32.0]);
            if t.start_solid {
                continue;
            }
            origin = t.end;
        }
        let triggers = [];
        if e.targetname != () {
            for o in entities {
                if o.classname != () && o.classname.to_lower() == "trigger_capture_area" && o.target == e.targetname && o.bounds != () {
                    triggers.push(o.bounds);
                }
            }
        }
        let explosions = [];
        if e.target != () {
            for o in entities {
                if o.targetname == () || o.classname == () || o.targetname != e.target {
                    continue;
                }
                let class = o.classname.to_lower();
                let dmg = 0.0;
                if o.dmg != () {
                    dmg = to_num(o.dmg, 0.0);
                }
                let radius = 0.0;
                if o.radius != () {
                    radius = to_num(o.radius, 0.0);
                }
                if radius <= 0.0 {
                    radius = dmg + 100.0;
                }
                let delay = 0.0;
                if o.delay != () {
                    delay = to_num(o.delay, 0.0);
                }
                if dmg < 1.0 {
                    dmg = 1.0;
                }
                if class == "target_explosion" && o.origin != () {
                    explosions.push(#{ origin: o.origin, radius: radius, damage: dmg, delay: ms(delay) });
                } else if class == "func_explosive" && o.bounds != () {
                    let center = vscale(vadd(o.bounds.mins, o.bounds.maxs), 0.5);
                    explosions.push(#{ origin: center, radius: radius, damage: dmg, delay: ms(delay) });
                }
            }
        }
        this.sites.push(#{ letter: letters[this.sites.len()], indicator: origin, triggers: triggers, explosions: explosions });
    }
}

fn start_match(g) {
    this.started = true;
    this.match_state = "countdown";
    this.state_since = g.now();
    this.match_started = g.now();
    g.set_movement_locked(true);
    g.emit("countdown", #{});
    for id in 0..g.player_count() {
        this.spawn_player(g, id);
    }
}

fn on_player_joined(g, id) {
    if this.started && this.match_state == "countdown" {
        this.spawn_player(g, id);
    }
}

// ---------------------------------------------------------------- players

fn ensure_loadout(id) {
    let key = id.to_string();
    if !this.loads.contains(key) {
        this.loads[key] = #{ primary: ["rl", "eb"], secondary: "pg" };
    }
    key
}

fn give_inventory(g, id) {
    let load = this.loads[this.ensure_loadout(id)];
    g.clear_weapons(id);
    g.give_weapon(id, "gunblade", 0);
    for weapon in load.primary {
        g.give_weapon(id, weapon, ammo_for(weapon));
    }
    g.give_weapon(id, load.secondary, ammo_for(load.secondary));
}

fn spawn_player(g, id) {
    let spawns = this.def_spawns;
    if g.team(id) == this.attacker {
        spawns = this.atk_spawns;
    }
    let free = [];
    for s in spawns {
        let taken = false;
        for p in 0..g.player_count() {
            if g.alive(p) && g.origin(p) == s.origin {
                taken = true;
                break;
            }
        }
        if !taken {
            free.push(s);
        }
    }
    let pool = spawns;
    if free.len() > 0 {
        pool = free;
    }
    let s = pool[g.rand(pool.len())];
    g.spawn_at(id, s.origin, s.yaw);
    this.give_inventory(g, id);
}

fn team_alive(g, team) {
    let n = 0;
    for id in 0..g.player_count() {
        if g.alive(id) && g.team(id) == team {
            n += 1;
        }
    }
    n
}

fn team_size(g, team) {
    let n = 0;
    for id in 0..g.player_count() {
        if g.team(id) == team {
            n += 1;
        }
    }
    n
}

fn on_command(g, id, name, args) {
    if name == "drop" {
        if this.bomb.carrier == id && this.round_state == "round" {
            this.release_bomb(g, id, true);
        }
        return true;
    }
    if name == "suicide" {
        this.on_damage(g, id, -1, g.health(id) + g.armor(id) + 1.0);
        return true;
    }
    if name == "carrier" {
        g.set_stat(id, "no_carrier", if args == "off" { 1 } else { 0 });
        return true;
    }
    if name == "primary" {
        let picks = args.split(" ");
        if picks.len() != 2 || picks[0] == picks[1] {
            return false;
        }
        for pick in picks {
            if !["rl", "lg", "eb"].contains(pick) {
                return false;
            }
        }
        this.loads[this.ensure_loadout(id)].primary = picks;
        this.regive(g, id);
        return true;
    }
    if name == "secondary" {
        if !["pg", "rg", "mg", "gl", "gb"].contains(args) {
            return false;
        }
        this.loads[this.ensure_loadout(id)].secondary = args;
        this.regive(g, id);
        return true;
    }
    false
}

// Selecting during the pre-round phase re-issues the inventory immediately.
fn regive(g, id) {
    if this.round_state == "pre" && g.alive(id) {
        this.give_inventory(g, id);
    }
}

fn on_team_change(g, id, team) {
    if g.team(id) == team {
        return;
    }
    if this.bomb.carrier == id {
        this.release_bomb(g, id, false);
    }
    g.set_team(id, team);
    if this.match_state == "playing" {
        g.ghost(id);
    }
    if this.match_state == "countdown" && this.started {
        this.spawn_player(g, id);
    }
    if this.match_state == "playing" && this.round_state == "round" {
        this.team_at_start = [this.team_size(g, 0), this.team_size(g, 1)];
        this.check_elimination(g);
    }
}

// ---------------------------------------------------------------- damage

fn spawn_protected(g) {
    (g.now() - this.round_started) / 1000 <= this.cfg.protect
}

fn on_damage(g, victim, attacker, amount) {
    if this.match_state != "playing" || !g.alive(victim) {
        return;
    }
    if this.round_state != "round" && this.round_state != "finished" {
        return;
    }
    if attacker >= 0 && attacker != victim && g.alive(attacker) && g.team(attacker) == g.team(victim)
        && this.round_state == "round" && this.spawn_protected(g) {
        this.punish(g, attacker, amount);
        return;
    }
    if g.apply_damage(victim, amount) {
        this.after_death(g, victim, attacker);
    }
}

// Teamdamage during spawn protection is reflected: harsher in the first third of the
// window, softer in the last.
fn punish(g, attacker, amount) {
    let protect = this.cfg.protect;
    let third = protect / 3;
    let elapsed = (g.now() - this.round_started) / 1000;
    let dmg = amount;
    if elapsed <= third {
        dmg *= 2.0;
    } else if elapsed >= protect - third {
        dmg *= 0.5;
    }
    g.set_armor(attacker, max(g.armor(attacker) - dmg * 0.5, 0.0));
    g.emit("punished", #{ player: attacker, damage: dmg });
    let health = g.health(attacker) - dmg;
    if health <= 0.0 {
        g.ghost(attacker);
        this.after_death(g, attacker, attacker);
    } else {
        g.set_health(attacker, health);
    }
}

fn after_death(g, victim, attacker) {
    g.set_stat(victim, "deaths", g.stat(victim, "deaths") + 1);
    if attacker >= 0 && attacker != victim {
        g.set_stat(attacker, "kills", g.stat(attacker, "kills") + 1);
    }
    g.emit("killed", #{ victim: victim, attacker: attacker });
    if this.bomb.carrier == victim {
        this.release_bomb(g, victim, true);
        this.bomb.velocity[2] *= 0.5;
    }
    if this.round_state == "round" {
        this.check_elimination(g);
    }
}

// Radius damage to every living player with a clear line from `origin`. The engine picks
// the victims (slot order, deterministic) and the falloff; the rules (`on_damage`) apply.
fn radius_damage(g, origin, radius, damage, falloff) {
    for hit in g.radius_damage(origin, radius, damage, falloff) {
        this.on_damage(g, hit.victim, -1, hit.amount);
    }
}

// ---------------------------------------------------------------- rounds

fn check_elimination(g) {
    let attacker = this.attacker;
    let defender = 1 - attacker;
    let attackers_out = this.team_at_start[attacker] > 0 && this.team_alive(g, attacker) == 0;
    let defenders_out = this.team_at_start[defender] > 0 && this.team_alive(g, defender) == 0;
    if defenders_out {
        this.round_won(g, attacker, "elimination");
    } else if attackers_out && this.bomb.state != "armed" {
        this.round_won(g, defender, "elimination");
    }
}

fn round_won(g, team, reason) {
    if this.round_state != "round" {
        return;
    }
    this.scores[team] += 1;
    this.round_state = "finished";
    this.state_since = g.now();
    g.emit("round_won", #{ team: team, reason: reason });
}

fn match_over() {
    let limit = this.cfg.score_limit;
    let a = this.scores[0];
    let b = this.scores[1];
    limit > 0 && max(a, b) >= limit && abs(a - b) > 1
}

fn end_match(g) {
    this.winner = -1;
    if this.scores[0] > this.scores[1] {
        this.winner = 0;
    } else if this.scores[1] > this.scores[0] {
        this.winner = 1;
    }
    this.match_state = "postmatch";
    this.state_since = g.now();
    g.set_movement_locked(true);
    g.emit("match_won", #{ team: this.winner });
}

fn begin_pre(g) {
    if swap_roles(this.round_count, this.cfg.score_limit) {
        this.attacker = 1 - this.attacker;
    }
    this.round_count += 1;
    this.match_state = "playing";
    this.round_state = "pre";
    this.state_since = g.now();
    this.pending = [];
    this.bomb = new_bomb();
    for id in 0..g.player_count() {
        g.ghost(id);
    }
    for id in 0..g.player_count() {
        this.spawn_player(g, id);
    }
    this.team_at_start = [this.team_size(g, 0), this.team_size(g, 1)];
    g.set_movement_locked(true);
    g.emit("round_starting", #{ round: this.round_count, attacker: this.attacker });
    this.give_bomb(g);
}

fn give_bomb(g) {
    let attackers = [];
    let preferred = [];
    for id in 0..g.player_count() {
        if g.team(id) == this.attacker {
            attackers.push(id);
            if this.cfg.carriers && g.stat(id, "no_carrier") == 0 {
                preferred.push(id);
            }
        }
    }
    if attackers.len() == 0 {
        return;
    }
    let pool = attackers;
    if preferred.len() > 0 {
        pool = preferred;
    }
    let carrier = pool[g.rand(pool.len())];
    this.bomb.state = "carried";
    this.bomb.carrier = carrier;
    g.emit("bomb_given", #{ player: carrier });
}

fn begin_round(g) {
    this.round_state = "round";
    this.round_started = g.now();
    this.state_since = g.now();
    g.set_movement_locked(false);
    g.emit("round_started", #{});
}

fn tick(g) {
    let now = g.now();
    if this.match_state == "postmatch" {
        return;
    }
    if this.match_state == "countdown" {
        if this.started && now - this.state_since >= this.cfg.countdown {
            this.begin_pre(g);
        }
        return;
    }
    if this.cfg.time_limit > 0 && now - this.match_started >= this.cfg.time_limit {
        this.end_match(g);
        return;
    }
    this.run_pending(g);
    if this.round_state == "pre" {
        if now - this.state_since >= 5000 {
            this.begin_round(g);
        }
    } else if this.round_state == "round" {
        this.tick_round(g);
    } else if this.round_state == "finished" {
        if now - this.state_since >= 1500 {
            this.round_state = "post";
            this.state_since = now;
        }
    } else if now - this.state_since >= 3000 {
        if this.match_over() {
            this.end_match(g);
        } else {
            this.begin_pre(g);
        }
    }
}

fn tick_round(g) {
    this.tick_bomb(g);
    if this.round_state != "round" {
        return;
    }
    if g.now() - this.round_started >= this.cfg.round && this.bomb.state != "armed" {
        this.round_won(g, 1 - this.attacker, "timeout");
    }
}

// ---------------------------------------------------------------- bomb

fn tick_bomb(g) {
    let state = this.bomb.state;
    if state == "carried" {
        if this.bomb.carrier < 0 {
            return;
        }
        this.bomb.origin = g.origin(this.bomb.carrier);
        this.try_plant(g, this.bomb.carrier);
    } else if state == "dropping" {
        this.step_dropping(g);
        this.check_lost(g);
        this.try_pickup(g);
    } else if state == "dropped" {
        this.check_lost(g);
        this.try_pickup(g);
    } else if state == "planting" || state == "planted" {
        if state == "planting" && g.now() - this.bomb.since >= 300 {
            this.bomb.state = "planted";
        }
        this.tick_arming(g);
    } else if state == "armed" {
        this.tick_armed(g);
    }
}

fn release_bomb(g, id, toss) {
    let o = g.origin(id);
    let f = g.forward(id);
    this.bomb.carrier = -1;
    this.bomb.dropper = id;
    this.bomb.retake_at = g.now() + 1000;
    this.bomb.since = g.now();
    if toss {
        let eye = vadd(o, [0.0, 0.0, 30.0]);
        this.bomb.origin = vadd(vadd(eye, vscale(f, 24.0)), [0.0, 0.0, -16.0]);
        let throw_velocity = vadd(g.velocity(id), vscale(f, 200.0));
        throw_velocity[2] = 300.0;
        this.bomb.velocity = throw_velocity;
        this.bomb.state = "dropping";
    } else {
        this.bomb.origin = o;
        this.bomb.velocity = [0.0, 0.0, 0.0];
        this.bomb.state = "dropped";
    }
    g.emit("bomb_dropped", #{ origin: this.bomb.origin });
}

// A bomb that leaves the world is lost, which hands the round to the defenders.
fn check_lost(g) {
    if !g.in_world(this.bomb.origin) {
        this.round_won(g, 1 - this.attacker, "bomb_lost");
    }
}

fn step_dropping(g) {
    let dt = 0.008;
    let v = this.bomb.velocity;
    v[2] -= 850.0 * dt;
    let end = vadd(this.bomb.origin, vscale(v, dt));
    let t = g.trace(this.bomb.origin, end, [-16.0, -16.0, -8.0], [16.0, 16.0, 48.0]);
    this.bomb.origin = t.end;
    this.bomb.velocity = v;
    let settled = g.now() - this.bomb.since >= 10000;
    if t.fraction < 1.0 || t.start_solid {
        if t.normal[2] > 0.7 || settled || t.start_solid {
            this.bomb.velocity = [0.0, 0.0, 0.0];
            this.bomb.state = "dropped";
        } else {
            let into = vdot(v, t.normal);
            this.bomb.velocity = vscale(vsub(v, vscale(t.normal, into * 1.5)), 0.5);
        }
    } else if settled {
        this.bomb.velocity = [0.0, 0.0, 0.0];
        this.bomb.state = "dropped";
    }
}

fn try_pickup(g) {
    let b = bomb_bounds(this.bomb.origin);
    for id in 0..g.player_count() {
        if !g.alive(id) || g.team(id) != this.attacker {
            continue;
        }
        if this.bomb.dropper == id && g.now() < this.bomb.retake_at {
            continue;
        }
        let p = player_bounds(g.origin(id));
        if overlaps(p.mins, p.maxs, b.mins, b.maxs) {
            this.bomb.state = "carried";
            this.bomb.carrier = id;
            g.emit("bomb_picked_up", #{ player: id });
            return;
        }
    }
}

fn on_ground(g, origin) {
    let t = g.trace(origin, vadd(origin, [0.0, 0.0, -1.0]), [-16.0, -16.0, -24.0], [16.0, 16.0, 40.0]);
    !t.start_solid && t.fraction < 1.0 && t.normal[2] >= 0.90
}

// A site is usable when the carrier touches its capture trigger; sites without a
// trigger fall back to proximity and line of sight to the indicator.
fn plant_site(g, carrier) {
    let origin = g.origin(carrier);
    let body = player_bounds(origin);
    for i in 0..this.sites.len() {
        let site = this.sites[i];
        if site.triggers.len() == 0 {
            if vdist(site.indicator, origin) <= 400.0 && g.los(vadd(site.indicator, [0.0, 0.0, 96.0]), origin) {
                return i;
            }
        } else {
            for trigger in site.triggers {
                if overlaps(trigger.mins, trigger.maxs, body.mins, body.maxs) {
                    return i;
                }
            }
        }
    }
    -1
}

fn try_plant(g, carrier) {
    let origin = g.origin(carrier);
    if vlen(g.velocity(carrier)) > 400.0 || !on_ground(g, origin) {
        return;
    }
    let site = this.plant_site(g, carrier);
    if site < 0 {
        return;
    }
    let t = g.trace(origin, vadd(origin, [0.0, 0.0, -512.0]), [-16.0, -16.0, -8.0], [16.0, 16.0, 48.0]);
    this.bomb.origin = t.end;
    this.bomb.carrier = -1;
    this.bomb.site = site;
    this.bomb.state = "planting";
    this.bomb.since = g.now();
    this.bomb.arm = 0;
    this.bomb.defuse = 0;
    g.set_stat(carrier, "plants", g.stat(carrier, "plants") + 1);
    g.emit("bomb_planted", #{ site: site, letter: this.sites[site].letter });
}

// Living players of `team` within 100 units of the bomb with a clear line to it.
fn nearby(g, team) {
    let center = vadd(this.bomb.origin, [0.0, 0.0, 8.0]);
    let found = [];
    for id in 0..g.player_count() {
        if g.alive(id) && g.team(id) == team && vdist(center, g.origin(id)) <= 100.0 && g.los(center, g.origin(id)) {
            found.push(id);
        }
    }
    found
}

fn tick_arming(g) {
    if this.nearby(g, this.attacker).len() > 0 {
        this.bomb.arm += 8;
    } else {
        this.bomb.arm = max(this.bomb.arm - 8, 0);
    }
    if this.bomb.arm >= this.cfg.arm {
        this.bomb.state = "armed";
        this.bomb.explode_at = g.now() + this.cfg.bomb;
        this.bomb.defuse = 0;
        g.emit("bomb_armed", #{ site: this.bomb.site, letter: this.sites[this.bomb.site].letter });
        this.check_elimination(g);
    }
}

fn tick_armed(g) {
    let site = this.bomb.site;
    let letter = this.sites[site].letter;
    let defenders = this.nearby(g, 1 - this.attacker);
    if defenders.len() == 0 {
        this.bomb.defuse = max(this.bomb.defuse - 8, 0);
    } else {
        this.bomb.defuse += 8;
    }
    // Defusing is checked before exploding so a tie goes to the defenders.
    if this.bomb.defuse >= this.cfg.defuse {
        for id in defenders {
            g.set_stat(id, "defuses", g.stat(id, "defuses") + 1);
        }
        this.bomb.state = "defused";
        g.emit("bomb_defused", #{ site: site, letter: letter });
        this.round_won(g, 1 - this.attacker, "bomb_defused");
        return;
    }
    if this.bomb.explode_at - g.now() <= 12000 && !this.bomb.hurry {
        this.bomb.hurry = true;
        g.emit("hurry_up", #{ team: 1 - this.attacker });
    }
    if g.now() >= this.bomb.explode_at {
        this.explode(g, site);
    }
}

fn explode(g, site) {
    this.bomb.state = "exploded";
    g.emit("bomb_exploded", #{ site: site, letter: this.sites[site].letter });
    // Score first so the blast's own kills cannot end the round a second time.
    this.round_won(g, this.attacker, "bomb_exploded");
    this.radius_damage(g, vadd(this.bomb.origin, [0.0, 0.0, 8.0]), 9001.0, 9001.0, false);
    for e in this.sites[site].explosions {
        this.pending.push(#{ at: g.now() + e.delay, origin: e.origin, radius: e.radius, damage: e.damage });
    }
}

fn run_pending(g) {
    if this.pending.len() == 0 {
        return;
    }
    let due = [];
    let later = [];
    for e in this.pending {
        if e.at <= g.now() {
            due.push(e);
        } else {
            later.push(e);
        }
    }
    this.pending = later;
    for e in due {
        this.radius_damage(g, e.origin, e.radius, e.damage, true);
    }
}

// ---------------------------------------------------------------- HUD

fn hud(g, id) {
    let now = g.now();
    let clock = 0;
    if this.match_state == "countdown" {
        clock = max(this.state_since + this.cfg.countdown - now, 0);
    } else if this.match_state == "playing" {
        if this.round_state == "pre" {
            clock = max(this.state_since + 5000 - now, 0);
        } else if this.round_state == "round" {
            if this.bomb.state == "armed" {
                clock = max(this.bomb.explode_at - now, 0);
            } else {
                clock = max(this.round_started + this.cfg.round - now, 0);
            }
        }
    }
    let progress = 0.0;
    if this.bomb.state == "planting" || this.bomb.state == "planted" {
        progress = this.bomb.arm.to_float() / this.cfg.arm.to_float();
    } else if this.bomb.state == "armed" {
        progress = this.bomb.defuse.to_float() / this.cfg.defuse.to_float();
    }
    let letter = "";
    if this.bomb.site >= 0 {
        letter = this.sites[this.bomb.site].letter;
    }
    let plant_letter = "";
    if this.bomb.carrier == id && this.bomb.state == "carried" {
        let s = this.plant_site(g, id);
        if s >= 0 {
            plant_letter = this.sites[s].letter;
        }
    }
    #{
        plant_site: plant_letter,
        match_state: this.match_state,
        round_state: this.round_state,
        round: this.round_count,
        clock_ms: clock,
        score_alpha: this.scores[0],
        score_beta: this.scores[1],
        team: g.team(id),
        attacker: g.team(id) == this.attacker,
        alive: g.alive(id),
        health: g.health(id),
        armor: g.armor(id),
        carrying: this.bomb.carrier == id,
        bomb: this.bomb.state,
        bomb_site: letter,
        progress: min(max(progress, 0.0), 1.0),
        winner: this.winner,
    }
}
