//! `.shader` script parser (Quake 3 / qfusion dialect).
//!
//! Semantics follow the legacy renderer (`source/client/renderer/shader.cpp`). Only the
//! commonly used subset is interpreted; everything else is skipped without error so that
//! stock Warsow scripts load. The module is GPU-free: it produces plain data that the
//! client turns into pipelines and bind groups.

use std::collections::HashMap;

use super::vfs::Vfs;

#[derive(Debug, Clone, PartialEq)]
pub enum MapSource {
    /// `$lightmap`: the BSP lightmap of the surface.
    Lightmap,
    /// `$whiteimage`.
    White,
    Image(String),
    /// `animmap <fps> <frames...>`.
    Anim {
        fps: f32,
        frames: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendFactor {
    Zero,
    One,
    SrcColor,
    OneMinusSrcColor,
    DstColor,
    OneMinusDstColor,
    SrcAlpha,
    OneMinusSrcAlpha,
    DstAlpha,
    OneMinusDstAlpha,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blend {
    pub src: BlendFactor,
    pub dst: BlendFactor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaFunc {
    Gt0,
    Lt128,
    Ge128,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveFunc {
    Sin,
    Triangle,
    Square,
    Sawtooth,
    InverseSawtooth,
    Noise,
    /// `wave distanceramp v0 v1 d0 d1`: `v0` up to distance `d0`, `v1` from `d1`, linear in
    /// between. The waveform fields hold `v0 v1 d0 d1` in `base amp phase freq` order.
    DistanceRamp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Waveform {
    pub func: WaveFunc,
    pub base: f32,
    pub amp: f32,
    pub phase: f32,
    pub freq: f32,
}

impl Waveform {
    /// Distance ramp value at `distance` units from the viewer.
    pub fn distance_ramp(&self, distance: f32) -> f32 {
        let (v0, v1, d0, d1) = (self.base, self.amp, self.phase, self.freq);
        let t = if d1 > d0 {
            ((distance - d0) / (d1 - d0)).clamp(0.0, 1.0)
        } else if distance >= d1 {
            1.0
        } else {
            0.0
        };
        v0 + (v1 - v0) * t
    }

    /// Value of the waveform at `time` seconds. Distance ramps have no per-surface viewer
    /// distance on the CPU, so they evaluate at infinity (the far value).
    pub fn eval(&self, time: f32) -> f32 {
        if self.func == WaveFunc::DistanceRamp {
            return self.distance_ramp(f32::INFINITY);
        }
        let t = self.phase + time * self.freq;
        let f = t - t.floor();
        let v = match self.func {
            WaveFunc::Sin => (f * std::f32::consts::TAU).sin(),
            WaveFunc::Triangle => {
                if f < 0.5 {
                    4.0 * f - 1.0
                } else {
                    3.0 - 4.0 * f
                }
            }
            WaveFunc::Square => {
                if f < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            WaveFunc::Sawtooth => f,
            WaveFunc::InverseSawtooth => 1.0 - f,
            WaveFunc::Noise | WaveFunc::DistanceRamp => 0.0,
        };
        self.base + v * self.amp
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RgbGen {
    Identity,
    IdentityLighting,
    Vertex,
    ExactVertex,
    OneMinusVertex,
    Const([f32; 3]),
    Entity,
    LightingDiffuse,
    Wave(Waveform),
    /// `teamcolor <n>`: the player's team colour (index 2 alpha, 3 beta). The client has no
    /// team colours yet and renders it as identity.
    TeamColor(u32),
    /// `teamcolorwave <n> <wave>`.
    TeamColorWave(u32, Waveform),
    /// `colorwave <r> <g> <b> <wave>`: constant colour scaled by the waveform.
    ColorWave([f32; 3], Waveform),
}

#[derive(Debug, Clone, PartialEq)]
pub enum AlphaGen {
    Identity,
    Vertex,
    Const(f32),
    Wave(Waveform),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TcGen {
    Base,
    Lightmap,
    Environment,
    Vector([f32; 3], [f32; 3]),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TcMod {
    Scroll(f32, f32),
    Scale(f32, f32),
    /// Degrees per second.
    Rotate(f32),
    Stretch(Waveform),
    Turb {
        base: f32,
        amp: f32,
        phase: f32,
        freq: f32,
    },
    Transform([f32; 6]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stage {
    pub map: MapSource,
    pub clamp: bool,
    pub blend: Option<Blend>,
    pub alpha_func: Option<AlphaFunc>,
    pub rgb_gen: RgbGen,
    pub alpha_gen: AlphaGen,
    pub tc_gen: TcGen,
    pub tc_mods: Vec<TcMod>,
    pub depth_write: bool,
    pub depth_equal: bool,
    /// Warsow `material` stage: the diffuse image lit by the lightmap.
    pub material: bool,
    /// Shader `polygonoffset`: pulled toward the viewer so coplanar decals don't z-fight.
    pub polygon_offset: bool,
}

impl Stage {
    pub fn new(map: MapSource) -> Self {
        Self {
            map,
            clamp: false,
            blend: None,
            alpha_func: None,
            rgb_gen: RgbGen::Identity,
            alpha_gen: AlphaGen::Identity,
            tc_gen: TcGen::Base,
            tc_mods: Vec::new(),
            depth_write: true,
            depth_equal: false,
            material: false,
            polygon_offset: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cull {
    #[default]
    Front,
    Back,
    None,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkyParms {
    /// Base path of the far box (`<base>_rt/_lf/_up/_dn/_bk/_ft`), if any.
    pub far_box: Option<String>,
    pub cloud_height: f32,
    pub near_box: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SurfaceParms {
    pub sky: bool,
    pub nodraw: bool,
    pub trans: bool,
    pub nonsolid: bool,
    pub nolightmap: bool,
    pub fog: bool,
    pub water: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shader {
    pub name: String,
    pub stages: Vec<Stage>,
    pub cull: Cull,
    pub parms: SurfaceParms,
    pub sky: Option<SkyParms>,
    pub sort: Option<f32>,
    /// True for shaders synthesized from a plain texture because no script exists.
    pub implicit: bool,
}

impl Shader {
    /// Legacy default shader for a world surface with no script: lightmap * texture.
    pub fn implicit(name: &str) -> Self {
        let mut lightmap = Stage::new(MapSource::Lightmap);
        lightmap.tc_gen = TcGen::Lightmap;
        let mut texture = Stage::new(MapSource::Image(name.to_string()));
        texture.blend = Some(Blend {
            src: BlendFactor::DstColor,
            dst: BlendFactor::Zero,
        });
        texture.depth_write = false;
        Self {
            name: name.to_string(),
            stages: vec![lightmap, texture],
            cull: Cull::Front,
            parms: SurfaceParms::default(),
            sky: None,
            sort: None,
            implicit: true,
        }
    }

    pub fn is_sky(&self) -> bool {
        self.parms.sky || self.sky.is_some()
    }

    /// Surfaces that must not be drawn at all.
    pub fn is_hidden(&self) -> bool {
        self.parms.nodraw
    }

    /// Whether the shader sorts after opaque geometry.
    pub fn is_translucent(&self) -> bool {
        self.parms.trans
            || self.sort.is_some_and(|s| s > 8.5)
            || (!self.stages.is_empty() && self.stages.iter().all(|s| s.blend.is_some()))
    }
}

// ---------------------------------------------------------------------------------------------
// Tokenizer

#[derive(Debug, PartialEq)]
enum Tok {
    Word(String),
    Open,
    Close,
    Newline,
}

fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut chars = text.chars().peekable();
    let mut word = String::new();
    fn flush(word: &mut String, out: &mut Vec<Tok>) {
        if !word.is_empty() {
            out.push(Tok::Word(std::mem::take(word)));
        }
    }
    while let Some(c) = chars.next() {
        match c {
            '/' if chars.peek() == Some(&'/') => {
                flush(&mut word, &mut out);
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(Tok::Newline);
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                flush(&mut word, &mut out);
                chars.next();
                let mut prev = '\0';
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(Tok::Newline);
                    }
                    if prev == '*' && c == '/' {
                        break;
                    }
                    prev = c;
                }
            }
            '"' => {
                flush(&mut word, &mut out);
                let mut quoted = String::new();
                for c in chars.by_ref() {
                    if c == '"' || c == '\n' {
                        break;
                    }
                    quoted.push(c);
                }
                out.push(Tok::Word(quoted));
            }
            '{' => {
                flush(&mut word, &mut out);
                out.push(Tok::Open);
            }
            '}' => {
                flush(&mut word, &mut out);
                out.push(Tok::Close);
            }
            '\n' => {
                flush(&mut word, &mut out);
                out.push(Tok::Newline);
            }
            c if c.is_whitespace() || c == '\0' => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

struct Cursor {
    toks: Vec<Tok>,
    pos: usize,
}

impl Cursor {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn skip_newlines(&mut self) {
        while self.peek() == Some(&Tok::Newline) {
            self.pos += 1;
        }
    }

    /// Words up to (not including) the end of the current directive.
    fn args(&mut self) -> Vec<String> {
        let mut args = Vec::new();
        while let Some(Tok::Word(w)) = self.peek() {
            args.push(w.clone());
            self.pos += 1;
        }
        args
    }

    fn skip_line(&mut self) {
        while matches!(self.peek(), Some(Tok::Word(_))) {
            self.pos += 1;
        }
    }

    /// Skips the rest of a false `if` block, including nested ones; the cursor ends after
    /// the matching `endif`.
    fn skip_to_endif(&mut self) {
        let mut depth = 1;
        let mut line_start = true;
        while let Some(tok) = self.peek() {
            match tok {
                Tok::Word(w) if line_start => {
                    if w.eq_ignore_ascii_case("if") {
                        depth += 1;
                    } else if w.eq_ignore_ascii_case("endif") {
                        depth -= 1;
                        if depth == 0 {
                            self.pos += 1;
                            return;
                        }
                    }
                    line_start = false;
                }
                Tok::Word(_) => {}
                _ => line_start = true,
            }
            self.pos += 1;
        }
    }
}

/// Capabilities assumed for `if` conditions: a GLSL-capable GPU with cube maps, no deluxe
/// maps and no portal captures.
fn condition_var(name: &str) -> Option<i64> {
    Some(match name.to_ascii_lowercase().as_str() {
        "glsl" | "texturecubemap" => 1,
        "deluxe" | "deluxemaps" | "portalmaps" => 0,
        "maxtexturesize" | "maxtexturecubemapsize" => 16384,
        "maxtextureunits" => 16,
        _ => return None,
    })
}

/// Evaluates a shader `if` condition left to right: `[!] var [op value]` clauses joined by
/// `&&` / `||`. Unknown variables are false.
fn eval_condition(args: &[String]) -> bool {
    let args: Vec<String> = args
        .iter()
        .filter(|a| !matches!(a.as_str(), "(" | ")"))
        .cloned()
        .collect();
    let args = &args[..];
    let mut result: Option<bool> = None;
    let mut join_or = false;
    let mut i = 0;
    while i < args.len() {
        let mut negate = false;
        let mut name = args[i].as_str();
        if let Some(rest) = name.strip_prefix('!') {
            negate = true;
            name = rest;
            if name.is_empty() {
                i += 1;
                name = args.get(i).map_or("", String::as_str);
            }
        }
        i += 1;
        let mut value = condition_var(name);
        if let (Some(op), Some(rhs)) = (
            args.get(i),
            args.get(i + 1).and_then(|v| v.parse::<i64>().ok()),
        ) && matches!(op.as_str(), "<" | "<=" | ">" | ">=" | "==" | "!=")
        {
            value = value.map(|v| match op.as_str() {
                "<" => (v < rhs) as i64,
                "<=" => (v <= rhs) as i64,
                ">" => (v > rhs) as i64,
                ">=" => (v >= rhs) as i64,
                "==" => (v == rhs) as i64,
                _ => (v != rhs) as i64,
            });
            i += 2;
        }
        let clause = value.is_some_and(|v| v != 0) != negate;
        result = Some(match result {
            None => clause,
            Some(prev) if join_or => prev || clause,
            Some(prev) => prev && clause,
        });
        match args.get(i).map(String::as_str) {
            Some("||") => {
                join_or = true;
                i += 1;
            }
            Some("&&") => {
                join_or = false;
                i += 1;
            }
            _ => {}
        }
    }
    result.unwrap_or(false)
}

/// Directives that are deliberately not interpreted. Everything else unknown produces a
/// warning, so what remains in `ShaderLib::warnings` is actionable.
///
/// Shader level: compile/editor hints (`qer_*`, `q3map_*`), texture filtering flags and
/// features the client does not implement yet (deform, portal, stencil,
/// fog volumes, soft particles, GLSL tuning).
pub const IGNORED_SHADER_DIRECTIVES: &[&str] = &[
    "nopicmip",
    "nomipmaps",
    "nocompress",
    "nofiltering",
    "smallestmipsize",
    "stenciltest",
    "deformvertexes",
    "portal",
    "entitymergable",
    "offsetmappingscale",
    "glossexponent",
    "glossintensity",
    "template",
    "softparticle",
    "forceworldoutlines",
    "fogparms",
    "fogparams",
    "skyparms2",
    "skyparmssides",
    "skip",
    "endif",
    "light",
    "tesssize",
    "forceoverlays",
    "noshadow",
    "flareshader",
    "detail",
    "smallestmipmapsize",
    "alphashadows",
    "nomodulativedlights",
];

/// Prefixes of ignored shader level directives (map compiler and editor keywords).
pub const IGNORED_SHADER_PREFIXES: &[&str] = &["qer_", "q3map_"];

/// Stage level directives that are deliberately not interpreted (cube maps, timelines,
/// distortion, cel shading and other effects the world pass does not implement).
pub const IGNORED_STAGE_DIRECTIVES: &[&str] = &[
    "cubemap",
    "shadecubemap",
    "timelinemap",
    "timelineclampmap",
    "timelinemap2",
    "timelineclampmap2",
    "distortion",
    "celshade",
    "detail",
    "greyscale",
    "grayscale",
    "surroundmap",
    "videomap",
    "skip",
    "endif",
];

// ---------------------------------------------------------------------------------------------
// Directive parsing

fn num(args: &[String], i: usize) -> Option<f32> {
    args.get(i)?.parse().ok()
}

fn waveform(args: &[String]) -> Option<Waveform> {
    let func = match args.first()?.to_ascii_lowercase().as_str() {
        "sin" => WaveFunc::Sin,
        "triangle" => WaveFunc::Triangle,
        "square" => WaveFunc::Square,
        "sawtooth" => WaveFunc::Sawtooth,
        "inversesawtooth" | "invsawtooth" => WaveFunc::InverseSawtooth,
        "noise" | "noize" => WaveFunc::Noise,
        "distanceramp" => WaveFunc::DistanceRamp,
        _ => return None,
    };
    Some(Waveform {
        func,
        base: num(args, 1)?,
        amp: num(args, 2)?,
        phase: num(args, 3)?,
        freq: num(args, 4)?,
    })
}

fn blend_factor(name: &str) -> Option<BlendFactor> {
    Some(match name.to_ascii_lowercase().trim_start_matches("gl_") {
        "zero" => BlendFactor::Zero,
        "one" => BlendFactor::One,
        "src_color" => BlendFactor::SrcColor,
        "one_minus_src_color" => BlendFactor::OneMinusSrcColor,
        "dst_color" => BlendFactor::DstColor,
        "one_minus_dst_color" => BlendFactor::OneMinusDstColor,
        "src_alpha" => BlendFactor::SrcAlpha,
        "one_minus_src_alpha" => BlendFactor::OneMinusSrcAlpha,
        "dst_alpha" => BlendFactor::DstAlpha,
        "one_minus_dst_alpha" => BlendFactor::OneMinusDstAlpha,
        _ => return None,
    })
}

fn parse_blend(args: &[String]) -> Option<Blend> {
    let first = args.first()?.to_ascii_lowercase();
    let (src, dst) = match first.as_str() {
        "add" => (BlendFactor::One, BlendFactor::One),
        "filter" => (BlendFactor::DstColor, BlendFactor::Zero),
        "blend" => (BlendFactor::SrcAlpha, BlendFactor::OneMinusSrcAlpha),
        // Not blend modes: the legacy parser falls back to additive for them.
        "decal" | "filterlighting" => (BlendFactor::One, BlendFactor::One),
        _ => (blend_factor(&first)?, blend_factor(args.get(1)?)?),
    };
    Some(Blend { src, dst })
}

fn parse_rgbgen(args: &[String]) -> Option<RgbGen> {
    Some(match args.first()?.to_ascii_lowercase().as_str() {
        "identity" => RgbGen::Identity,
        "identitylighting" | "lightingidentity" => RgbGen::IdentityLighting,
        "vertex" => RgbGen::Vertex,
        "exactvertex" => RgbGen::ExactVertex,
        "oneminusvertex" => RgbGen::OneMinusVertex,
        "entity" => RgbGen::Entity,
        "lightingdiffuse" => RgbGen::LightingDiffuse,
        "teamcolor" | "custom" => RgbGen::TeamColor(args.get(1)?.parse().ok()?),
        "teamcolorwave" | "customwave" => {
            RgbGen::TeamColorWave(args.get(1)?.parse().ok()?, waveform(&args[2..])?)
        }
        "colorwave" => RgbGen::ColorWave(
            [num(args, 1)?, num(args, 2)?, num(args, 3)?],
            waveform(&args[4..])?,
        ),
        "const" | "constant" => {
            let inner: Vec<String> = args[1..]
                .iter()
                .filter(|a| *a != "(" && *a != ")")
                .map(|a| a.trim_matches(['(', ')']).to_string())
                .filter(|a| !a.is_empty())
                .collect();
            RgbGen::Const([num(&inner, 0)?, num(&inner, 1)?, num(&inner, 2)?])
        }
        "wave" => RgbGen::Wave(waveform(&args[1..])?),
        _ => return None,
    })
}

fn parse_alphagen(args: &[String]) -> Option<AlphaGen> {
    Some(match args.first()?.to_ascii_lowercase().as_str() {
        "vertex" | "exactvertex" => AlphaGen::Vertex,
        "const" | "constant" => AlphaGen::Const(num(args, 1)?),
        "wave" => AlphaGen::Wave(waveform(&args[1..])?),
        _ => AlphaGen::Identity,
    })
}

fn parse_tcmod(args: &[String]) -> Option<TcMod> {
    Some(match args.first()?.to_ascii_lowercase().as_str() {
        "scroll" => TcMod::Scroll(num(args, 1)?, num(args, 2)?),
        "scale" => TcMod::Scale(num(args, 1)?, num(args, 2)?),
        "rotate" => TcMod::Rotate(num(args, 1)?),
        "stretch" => TcMod::Stretch(waveform(&args[1..])?),
        "turb" => {
            // Both `turb base amp phase freq` and the older `turb amp phase freq` exist.
            if args.len() >= 5 {
                TcMod::Turb {
                    base: num(args, 1)?,
                    amp: num(args, 2)?,
                    phase: num(args, 3)?,
                    freq: num(args, 4)?,
                }
            } else {
                TcMod::Turb {
                    base: 0.0,
                    amp: num(args, 1)?,
                    phase: num(args, 2)?,
                    freq: num(args, 3)?,
                }
            }
        }
        "transform" => TcMod::Transform([
            num(args, 1)?,
            num(args, 2)?,
            num(args, 3)?,
            num(args, 4)?,
            num(args, 5)?,
            num(args, 6)?,
        ]),
        _ => return None,
    })
}

fn parse_tcgen(args: &[String]) -> Option<TcGen> {
    Some(match args.first()?.to_ascii_lowercase().as_str() {
        "base" | "texture" => TcGen::Base,
        "lightmap" => TcGen::Lightmap,
        "environment" => TcGen::Environment,
        "vector" => {
            let inner: Vec<String> = args[1..]
                .iter()
                .flat_map(|a| a.split(['(', ')']).map(str::to_string).collect::<Vec<_>>())
                .filter(|a| !a.is_empty())
                .collect();
            TcGen::Vector(
                [num(&inner, 0)?, num(&inner, 1)?, num(&inner, 2)?],
                [num(&inner, 3)?, num(&inner, 4)?, num(&inner, 5)?],
            )
        }
        _ => return None,
    })
}

fn parse_map(args: &[String]) -> Option<MapSource> {
    let name = args.first()?;
    Some(match name.to_ascii_lowercase().as_str() {
        "$lightmap" => MapSource::Lightmap,
        "$whiteimage" | "*white" => MapSource::White,
        // Dynamic render targets and the like have no static image.
        n if n.starts_with('$') => MapSource::White,
        _ => MapSource::Image(name.clone()),
    })
}

/// Parses one stage body; the cursor sits just after the opening brace.
fn parse_stage(cur: &mut Cursor, warnings: &mut Vec<String>, shader: &str) -> Stage {
    let mut stage = Stage::new(MapSource::White);
    let mut explicit_depth_write = false;
    loop {
        cur.skip_newlines();
        match cur.peek() {
            None | Some(Tok::Close) => {
                cur.pos += 1;
                break;
            }
            Some(Tok::Open) => {
                cur.pos += 1;
                continue;
            }
            Some(Tok::Newline) => unreachable!(),
            Some(Tok::Word(key)) => {
                let key = key.to_ascii_lowercase();
                cur.pos += 1;
                let args = cur.args();
                if key == "if" {
                    if !eval_condition(&args) {
                        cur.skip_to_endif();
                    }
                    continue;
                }
                // `$1`-style arguments belong to unexpanded `template` shaders.
                let templated = args.iter().any(|a| a.starts_with('$'));
                let mut bad = |what: &str| {
                    if !templated {
                        warnings.push(format!("{shader}: malformed {what} `{}`", args.join(" ")));
                    }
                };
                match key.as_str() {
                    "map" | "clampmap" => match parse_map(&args) {
                        Some(map) => {
                            stage.map = map;
                            stage.clamp = key == "clampmap";
                        }
                        None => bad(&key),
                    },
                    "animmap" | "animclampmap" => match num(&args, 0) {
                        Some(fps) if args.len() > 1 => {
                            stage.map = MapSource::Anim {
                                fps,
                                frames: args[1..].to_vec(),
                            };
                            stage.clamp = key == "animclampmap";
                        }
                        _ => bad(&key),
                    },
                    "blendfunc" => match parse_blend(&args) {
                        Some(b) => stage.blend = Some(b),
                        None => bad("blendfunc"),
                    },
                    "alphafunc" => {
                        stage.alpha_func = match args.first().map(|a| a.to_ascii_uppercase()) {
                            Some(a) if a == "GT0" => Some(AlphaFunc::Gt0),
                            Some(a) if a == "LT128" => Some(AlphaFunc::Lt128),
                            Some(a) if a == "GE128" => Some(AlphaFunc::Ge128),
                            _ => {
                                bad("alphafunc");
                                None
                            }
                        }
                    }
                    "rgbgen" => match parse_rgbgen(&args) {
                        Some(g) => stage.rgb_gen = g,
                        None => bad("rgbgen"),
                    },
                    "alphagen" => match parse_alphagen(&args) {
                        Some(g) => stage.alpha_gen = g,
                        None => bad("alphagen"),
                    },
                    "tcgen" | "texgen" => match parse_tcgen(&args) {
                        Some(g) => stage.tc_gen = g,
                        None => bad("tcgen"),
                    },
                    "tcmod" => match parse_tcmod(&args) {
                        Some(m) => stage.tc_mods.push(m),
                        None => bad("tcmod"),
                    },
                    "material" => {
                        // Without arguments the shader's own name is the diffuse image.
                        stage.map = MapSource::Image(
                            args.first().cloned().unwrap_or_else(|| shader.to_string()),
                        );
                        stage.material = true;
                    }
                    "depthwrite" => explicit_depth_write = true,
                    "depthfunc" => {
                        stage.depth_equal = args
                            .first()
                            .is_some_and(|a| a.eq_ignore_ascii_case("equal"));
                    }
                    k if IGNORED_STAGE_DIRECTIVES.contains(&k)
                        || IGNORED_SHADER_PREFIXES.iter().any(|p| k.starts_with(p))
                        || k.starts_with('$') => {}
                    _ => warnings.push(format!("{shader}: unknown stage directive `{key}`")),
                }
            }
        }
    }
    stage.depth_write = explicit_depth_write || (stage.blend.is_none());
    stage
}

fn parse_shader_body(cur: &mut Cursor, name: &str, warnings: &mut Vec<String>) -> Shader {
    let mut shader = Shader {
        name: name.to_string(),
        stages: Vec::new(),
        cull: Cull::Front,
        parms: SurfaceParms::default(),
        sky: None,
        sort: None,
        implicit: false,
    };
    let mut polygon_offset = false;
    loop {
        cur.skip_newlines();
        match cur.peek() {
            None => break,
            Some(Tok::Close) => {
                cur.pos += 1;
                break;
            }
            Some(Tok::Open) => {
                cur.pos += 1;
                let stage = parse_stage(cur, warnings, name);
                if stage.material {
                    // Same layering as the implicit shader: lightmap, then diffuse filter.
                    let mut lit = Stage::new(MapSource::Lightmap);
                    lit.tc_gen = TcGen::Lightmap;
                    shader.stages.push(lit);
                    let mut diffuse = stage;
                    diffuse.blend = Some(Blend {
                        src: BlendFactor::DstColor,
                        dst: BlendFactor::Zero,
                    });
                    diffuse.depth_write = false;
                    shader.stages.push(diffuse);
                } else {
                    shader.stages.push(stage);
                }
            }
            Some(Tok::Newline) => unreachable!(),
            Some(Tok::Word(key)) => {
                let key = key.to_ascii_lowercase();
                cur.pos += 1;
                let args = cur.args();
                if key == "if" {
                    if !eval_condition(&args) {
                        cur.skip_to_endif();
                    }
                    continue;
                }
                match key.as_str() {
                    "cull" => {
                        shader.cull = match args.first().map(|a| a.to_ascii_lowercase()) {
                            Some(a) if matches!(a.as_str(), "none" | "twosided" | "disable") => {
                                Cull::None
                            }
                            Some(a) if matches!(a.as_str(), "back" | "backside" | "backsided") => {
                                Cull::Back
                            }
                            _ => Cull::Front,
                        }
                    }
                    "surfaceparm" => {
                        if let Some(p) = args.first() {
                            match p.to_ascii_lowercase().as_str() {
                                "sky" => shader.parms.sky = true,
                                "nodraw" | "skip" => shader.parms.nodraw = true,
                                "trans" | "translucent" => shader.parms.trans = true,
                                "nonsolid" => shader.parms.nonsolid = true,
                                "nolightmap" => shader.parms.nolightmap = true,
                                "fog" => shader.parms.fog = true,
                                "water" => shader.parms.water = true,
                                _ => {}
                            }
                        }
                    }
                    "skyparms" => {
                        let part = |i: usize| {
                            args.get(i)
                                .filter(|a| a.as_str() != "-")
                                .map(|a| a.to_string())
                        };
                        shader.sky = Some(SkyParms {
                            far_box: part(0),
                            cloud_height: num(&args, 1).unwrap_or(128.0),
                            near_box: part(2),
                        });
                        shader.parms.sky = true;
                    }
                    "polygonoffset" => polygon_offset = true,
                    "sort" => {
                        shader.sort = args.first().and_then(|a| {
                            a.parse().ok().or(match a.to_ascii_lowercase().as_str() {
                                "portal" => Some(2.0),
                                "sky" => Some(2.0),
                                "opaque" => Some(3.0),
                                "decal" => Some(4.0),
                                "banner" => Some(6.0),
                                "underwater" => Some(8.0),
                                "additive" => Some(9.0),
                                "nearest" => Some(16.0),
                                _ => None,
                            })
                        });
                    }
                    k if IGNORED_SHADER_DIRECTIVES.contains(&k)
                        || IGNORED_SHADER_PREFIXES.iter().any(|p| k.starts_with(p)) => {}
                    _ => warnings.push(format!("{name}: unknown directive `{key}`")),
                }
            }
        }
    }
    if polygon_offset {
        shader.sort.get_or_insert(4.0);
        for stage in &mut shader.stages {
            stage.polygon_offset = true;
        }
    }
    shader
}

// ---------------------------------------------------------------------------------------------
// Library

#[derive(Default)]
pub struct ShaderLib {
    shaders: HashMap<String, Shader>,
    pub warnings: Vec<String>,
}

fn key(name: &str) -> String {
    name.replace('\\', "/").to_ascii_lowercase()
}

impl ShaderLib {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.shaders.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shaders.is_empty()
    }

    /// Parses a script; the first definition of a name wins. Returns shaders added.
    pub fn parse_script(&mut self, text: &str, source: &str) -> usize {
        let text = text.trim_start_matches('\u{feff}');
        let mut cur = Cursor {
            toks: tokenize(text),
            pos: 0,
        };
        let mut added = 0;
        loop {
            cur.skip_newlines();
            let Some(tok) = cur.peek() else { break };
            let Tok::Word(name) = tok else {
                // Stray brace at top level.
                cur.pos += 1;
                continue;
            };
            let name = name.clone();
            cur.pos += 1;
            if name.eq_ignore_ascii_case("endif") {
                cur.skip_line();
                continue;
            }
            if name.eq_ignore_ascii_case("if") {
                let args = cur.args();
                if !eval_condition(&args) {
                    cur.skip_to_endif();
                }
                continue;
            }
            cur.skip_line();
            cur.skip_newlines();
            if cur.peek() != Some(&Tok::Open) {
                self.warnings
                    .push(format!("{source}: shader `{name}` has no body"));
                continue;
            }
            cur.pos += 1;
            let shader = parse_shader_body(&mut cur, &name, &mut self.warnings);
            if self.shaders.insert_if_absent(key(&name), shader) {
                added += 1;
            }
        }
        added
    }

    /// Loads every `scripts/*.shader` in the VFS (alphabetical, first definition wins).
    pub fn load_vfs(&mut self, vfs: &Vfs) -> usize {
        let mut total = 0;
        for name in vfs.list("scripts/") {
            if !name.ends_with(".shader") || name["scripts/".len()..].contains('/') {
                continue;
            }
            match vfs.read(&name) {
                Ok(data) => total += self.parse_script(&String::from_utf8_lossy(&data), &name),
                Err(e) => self.warnings.push(format!("{name}: {e}")),
            }
        }
        total
    }

    pub fn get(&self, name: &str) -> Option<&Shader> {
        self.shaders.get(&key(name))
    }

    /// Script shader if one exists, otherwise the implicit plain-texture shader.
    pub fn resolve(&self, name: &str) -> Shader {
        self.get(name)
            .cloned()
            .unwrap_or_else(|| Shader::implicit(name))
    }
}

trait InsertIfAbsent {
    fn insert_if_absent(&mut self, key: String, shader: Shader) -> bool;
}

impl InsertIfAbsent for HashMap<String, Shader> {
    fn insert_if_absent(&mut self, key: String, shader: Shader) -> bool {
        match self.entry(key) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(shader);
                true
            }
            std::collections::hash_map::Entry::Occupied(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = r#"
// comment
textures/a/wall
{
    qer_editorimage textures/a/wall.tga
    surfaceparm nonsolid
    cull none
    {
        map $lightmap
        rgbgen identity
    }
    {
        map textures/a/wall.tga   // trailing
        blendfunc filter
        tcmod scroll 0.5 -0.25
        tcmod rotate 30
        tcgen environment
    }
}

textures/a/glass { surfaceparm trans
{
clampmap textures/g.tga
blendfunc GL_SRC_ALPHA GL_ONE_MINUS_SRC_ALPHA
alphafunc GE128
rgbgen const ( 1 0.5 0 )
} }

textures/sky/s
{
    skyparms env/s 512 -
    surfaceparm sky
}

textures/a/anim { {
animmap 8 a.tga b.tga c.tga
rgbgen wave sin 0.5 0.5 0 2
} }
textures/a/wall { { map dupe.tga } }
textures/a/hidden { surfaceparm nodraw }
/* block
comment */
textures/a/after { { map after.tga } }
"#;

    fn lib() -> ShaderLib {
        let mut lib = ShaderLib::new();
        lib.parse_script(SCRIPT, "test");
        lib
    }

    #[test]
    fn parses_stages_and_globals() {
        let lib = lib();
        let wall = lib.get("Textures/A/Wall").unwrap();
        assert_eq!(wall.cull, Cull::None);
        assert!(wall.parms.nonsolid);
        assert_eq!(wall.stages.len(), 2);
        assert_eq!(wall.stages[0].map, MapSource::Lightmap);
        assert!(wall.stages[0].blend.is_none());
        assert!(wall.stages[0].depth_write);
        let s = &wall.stages[1];
        assert_eq!(s.map, MapSource::Image("textures/a/wall.tga".into()));
        assert_eq!(
            s.blend,
            Some(Blend {
                src: BlendFactor::DstColor,
                dst: BlendFactor::Zero
            })
        );
        assert!(!s.depth_write);
        assert_eq!(s.tc_gen, TcGen::Environment);
        assert_eq!(
            s.tc_mods,
            vec![TcMod::Scroll(0.5, -0.25), TcMod::Rotate(30.0)]
        );
    }

    #[test]
    fn material_stage_expands_to_lightmap_plus_diffuse() {
        let mut lib = ShaderLib::new();
        lib.parse_script(
            "textures/m/stucco\n{\n{\nmaterial textures/m/stucco.tga textures/m/stucco_norm.tga\n}\n}\n",
            "test",
        );
        let s = lib.get("textures/m/stucco").unwrap();
        assert_eq!(s.stages.len(), 2);
        assert_eq!(s.stages[0].map, MapSource::Lightmap);
        assert_eq!(
            s.stages[1].map,
            MapSource::Image("textures/m/stucco.tga".into())
        );
        assert!(s.stages[1].blend.is_some() && !s.stages[1].depth_write);
    }

    #[test]
    fn parses_single_line_shader_with_alpha_and_const_rgb() {
        let lib = lib();
        let glass = lib.get("textures/a/glass").unwrap();
        assert!(glass.parms.trans);
        assert!(glass.is_translucent());
        let s = &glass.stages[0];
        assert!(s.clamp);
        assert_eq!(s.alpha_func, Some(AlphaFunc::Ge128));
        assert_eq!(s.rgb_gen, RgbGen::Const([1.0, 0.5, 0.0]));
        assert_eq!(
            s.blend,
            Some(Blend {
                src: BlendFactor::SrcAlpha,
                dst: BlendFactor::OneMinusSrcAlpha
            })
        );
    }

    #[test]
    fn parses_sky_anim_and_nodraw() {
        let lib = lib();
        let sky = lib.get("textures/sky/s").unwrap();
        assert!(sky.is_sky());
        let p = sky.sky.as_ref().unwrap();
        assert_eq!(p.far_box.as_deref(), Some("env/s"));
        assert_eq!(p.cloud_height, 512.0);
        assert_eq!(p.near_box, None);

        let anim = lib.get("textures/a/anim").unwrap();
        match &anim.stages[0].map {
            MapSource::Anim { fps, frames } => {
                assert_eq!(*fps, 8.0);
                assert_eq!(frames, &["a.tga", "b.tga", "c.tga"]);
            }
            other => panic!("unexpected {other:?}"),
        }
        let RgbGen::Wave(w) = &anim.stages[0].rgb_gen else {
            panic!("expected wave");
        };
        assert!((w.eval(0.125) - 1.0).abs() < 1e-5);

        assert!(lib.get("textures/a/hidden").unwrap().is_hidden());
    }

    #[test]
    fn first_definition_wins_and_comments_are_skipped() {
        let lib = lib();
        let wall = lib.get("textures/a/wall").unwrap();
        assert_eq!(wall.stages.len(), 2, "duplicate must not override");
        assert!(lib.get("textures/a/after").is_some());
        assert_eq!(lib.len(), 6);
    }

    #[test]
    fn unknown_shader_resolves_to_implicit_texture() {
        let lib = lib();
        let s = lib.resolve("textures/none/missing");
        assert!(s.implicit);
        assert_eq!(s.stages.len(), 2);
        assert_eq!(s.stages[0].map, MapSource::Lightmap);
        assert_eq!(
            s.stages[1].map,
            MapSource::Image("textures/none/missing".into())
        );
    }

    #[test]
    fn malformed_directives_warn_without_failing() {
        let mut lib = ShaderLib::new();
        lib.parse_script(
            "x {\n{\nmap\nblendfunc bogus\nrgbgen const\ntcmod nope\n}\n}",
            "t",
        );
        assert!(lib.get("x").is_some());
        assert!(!lib.warnings.is_empty());
    }

    fn stage_of(script: &str, name: &str) -> (ShaderLib, Vec<Stage>) {
        let mut lib = ShaderLib::new();
        lib.parse_script(script, "t");
        let stages = lib.get(name).unwrap().stages.clone();
        (lib, stages)
    }

    #[test]
    fn rgbgen_teamcolor_and_lightingidentity() {
        let (lib, st) = stage_of(
            "a {\n{\nmap a.tga\nrgbgen teamcolor 2\n}\n{\nmap b.tga\nrgbgen lightingidentity\n}\n{\nmap c.tga\nrgbgen teamcolorwave 3 sin 0.5 0.5 0 1\n}\n}",
            "a",
        );
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
        assert!(matches!(st[0].rgb_gen, RgbGen::TeamColor(_)));
        assert_eq!(st[1].rgb_gen, RgbGen::IdentityLighting);
    }

    #[test]
    fn distanceramp_and_extra_waveforms_parse() {
        let (lib, st) = stage_of(
            "a {\n{\nmap a.tga\nalphagen distanceramp 0 1 100 200\n}\n{\nmap b.tga\nrgbgen wave invsawtooth 0 1 0 1\n}\n{\nmap c.tga\nrgbgen wave noize 0 1 0 1\n}\n}",
            "a",
        );
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
        assert_eq!(st.len(), 3);
    }

    #[test]
    fn material_without_args_uses_shader_name() {
        let (_, st) = stage_of("textures/m/x {\n{\nmaterial\n}\n}", "textures/m/x");
        assert!(
            st.iter()
                .any(|s| s.map == MapSource::Image("textures/m/x".into()))
        );
    }

    #[test]
    fn decal_blendfunc_is_accepted() {
        let (lib, st) = stage_of("a {\n{\nmap a.tga\nblendfunc decal\n}\n}", "a");
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
        assert!(st[0].blend.is_some());
    }

    #[test]
    fn polygonoffset_marks_stages_and_defaults_sort_to_decal() {
        let (lib, st) = stage_of("a {\npolygonoffset\n{\nmap a.tga\n}\n}", "a");
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
        assert!(st[0].polygon_offset);
        assert_eq!(lib.resolve("a").sort, Some(4.0));
        let (_, plain) = stage_of("b {\n{\nmap b.tga\n}\n}", "b");
        assert!(!plain[0].polygon_offset);
    }

    #[test]
    fn conditionals_select_branches() {
        let script = "a {\nif ( maxtexturesize > 8192 )\n{\nmap big.tga\n}\nendif\nif deluxe\n{\nmap deluxe.tga\n}\nendif\n{\nmap tail.tga\n}\n}\nb {\ncull none\n}\n";
        let mut lib = ShaderLib::new();
        lib.parse_script(script, "t");
        let maps: Vec<_> = lib
            .get("a")
            .unwrap()
            .stages
            .iter()
            .map(|s| s.map.clone())
            .collect();
        assert!(maps.contains(&MapSource::Image("big.tga".into())));
        assert!(!maps.contains(&MapSource::Image("deluxe.tga".into())));
        assert!(maps.contains(&MapSource::Image("tail.tga".into())));
        assert!(lib.get("b").is_some());
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
    }

    #[test]
    fn bom_is_stripped() {
        let mut lib = ShaderLib::new();
        lib.parse_script("\u{feff}a { cull none }", "t");
        assert!(lib.get("a").is_some());
    }

    #[test]
    fn ignored_directives_are_silent_and_unknown_ones_warn() {
        let mut lib = ShaderLib::new();
        lib.parse_script(
            "a {\nforceoverlays\nnoshadow\nq3map_foo 1\n{\nmap a.tga\nvideomap x\nq3map_bar\n$2\n}\n}\nb {\nbogusdirective\n{\nmap a.tga\nbogusstage\n}\n}",
            "t",
        );
        assert_eq!(lib.warnings.len(), 2, "{:?}", lib.warnings);
        assert!(lib.warnings[0].contains("bogusdirective"));
        assert!(lib.warnings[1].contains("bogusstage"));
    }

    #[test]
    fn templated_arguments_do_not_warn() {
        let mut lib = ShaderLib::new();
        lib.parse_script("a {\n{\nmap $1\nrgbgen $2\n}\n}", "t");
        assert!(lib.warnings.is_empty(), "{:?}", lib.warnings);
    }
}
