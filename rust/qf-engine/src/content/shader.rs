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
    /// Value of the waveform at `time` seconds.
    pub fn eval(&self, time: f32) -> f32 {
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
            WaveFunc::Noise => 0.0,
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
}

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
        "inversesawtooth" => WaveFunc::InverseSawtooth,
        "noise" => WaveFunc::Noise,
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
        _ => (blend_factor(&first)?, blend_factor(args.get(1)?)?),
    };
    Some(Blend { src, dst })
}

fn parse_rgbgen(args: &[String]) -> Option<RgbGen> {
    Some(match args.first()?.to_ascii_lowercase().as_str() {
        "identity" => RgbGen::Identity,
        "identitylighting" => RgbGen::IdentityLighting,
        "vertex" => RgbGen::Vertex,
        "exactvertex" => RgbGen::ExactVertex,
        "oneminusvertex" => RgbGen::OneMinusVertex,
        "entity" => RgbGen::Entity,
        "lightingdiffuse" => RgbGen::LightingDiffuse,
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
                let mut bad = |what: &str| {
                    warnings.push(format!("{shader}: malformed {what} `{}`", args.join(" ")));
                };
                match key.as_str() {
                    "map" | "clampmap" => match parse_map(&args) {
                        Some(map) => {
                            stage.map = map;
                            stage.clamp = key == "clampmap";
                        }
                        None => bad(&key),
                    },
                    "animmap" => match num(&args, 0) {
                        Some(fps) if args.len() > 1 => {
                            stage.map = MapSource::Anim {
                                fps,
                                frames: args[1..].to_vec(),
                            };
                        }
                        _ => bad("animmap"),
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
                    "depthwrite" => explicit_depth_write = true,
                    "depthfunc" => {
                        stage.depth_equal = args
                            .first()
                            .is_some_and(|a| a.eq_ignore_ascii_case("equal"));
                    }
                    _ => {}
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
                shader.stages.push(stage);
            }
            Some(Tok::Newline) => unreachable!(),
            Some(Tok::Word(key)) => {
                let key = key.to_ascii_lowercase();
                cur.pos += 1;
                let args = cur.args();
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
                    "sort" => {
                        shader.sort = args.first().and_then(|a| {
                            a.parse().ok().or(match a.to_ascii_lowercase().as_str() {
                                "portal" => Some(2.0),
                                "sky" => Some(2.0),
                                "opaque" => Some(3.0),
                                "banner" => Some(6.0),
                                "underwater" => Some(8.0),
                                "additive" => Some(9.0),
                                "nearest" => Some(16.0),
                                _ => None,
                            })
                        });
                    }
                    _ => {}
                }
            }
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
}
