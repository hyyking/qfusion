// Replays a Rust-port movement recording (docs/movement-recording.md) through the original
// gs_pmove.cpp against the real BSP collision model (cm_*.cpp), and writes the result in the
// same recording format so the Rust comparison tool can diff it tick by tick.
//
//   pmove_replay --recording run.qfrec --bsp map.bsp --out legacy.qfrec
//                [--trace-from N --trace-to M --trace-out trace.txt]
//
// With --trace-*, every collision query made during ticks N..M is logged for the comparison
// tool to classify a divergence as a collision or an integration mismatch.

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <vector>

#include <common/facilities/cmodel.h>
#include <common/facilities/gs_public.h>
#include <common/facilities/cvar.h>
#include <common/facilities/fscompat.h>
#include <common/facilities/messagestreams.h>
#include <common/facilities/syspublic.h>
#include <common/facilities/wswfs.h>
#include <common/helpers/q_libc.h>

// ---- Engine services the collision model and movement code expect -------------------------

namespace {
std::vector<char> loadedFile;
}

void Sys_Error( const char *format, ... ) {
	va_list args;
	va_start( args, format );
	std::vfprintf( stderr, format, args );
	va_end( args );
	std::abort();
}

void Com_Error( com_error_code_t, const char *format, ... ) {
	va_list args;
	va_start( args, format );
	std::fprintf( stderr, "Com_Error: " );
	std::vfprintf( stderr, format, args );
	std::fprintf( stderr, "\n" );
	va_end( args );
	std::exit( 3 );
}

void Com_Printf( const char *, ... ) {}

float Cvar_Value( const char * ) {
	return 0.0f;
}

cvar_t *Cvar_ForceSet( const char *, const char * ) {
	return nullptr;
}

unsigned Sys_GetProcessorFeatures() {
	return 0;
}

int FS_LoadFileExt( const char *path, int, void **buffer, void *, size_t, const char *, int ) {
	std::ifstream file( path, std::ios::binary );
	if( !file ) {
		return -1;
	}
	loadedFile.assign( std::istreambuf_iterator<char>( file ), std::istreambuf_iterator<char>() );
	if( buffer ) {
		*buffer = loadedFile.data();
	}
	return (int)loadedFile.size();
}

void FS_FreeFile( void * ) {}
int FS_FOpenFile( const char *, int *, int ) { return -1; }
void FS_FCloseFile( int ) {}
int FS_Read( void *, size_t, int ) { return 0; }
int FS_Write( const void *, size_t, int ) { return 0; }
int FS_Seek( int, int, int ) { return 0; }
cvar_t *Cvar_Get( const char *, const char *, cvar_flag_t, DeclaredConfigVar * ) { return nullptr; }

namespace wsw {
// Diagnostics are discarded; messages go to a throwaway buffer.
auto createRegularMessageStream( MessageDomain domain, MessageCategory category ) -> RegularMessageStream * {
	return new RegularMessageStream( new char[1024], 1024, domain, category );
}
void submitRegularMessageStream( RegularMessageStream * ) {}
namespace fs {
auto openAsReadHandle( const wsw::StringView &, CacheUsage ) -> std::optional<ReadHandle> { return std::nullopt; }
}
}

// ---- Recording model ---------------------------------------------------------------------

namespace {

struct Tick {
	int msec, buttons, forward, side, up, angles[3];
	float origin[3], velocity[3];
	int flags, pmTime;
	float viewHeight, maxsZ;
	int delta[3];
	bool trigger;
};

struct Recording {
	std::vector<std::string> header;
	std::string columns;
	float origin[3] {}, velocity[3] {}, viewAngles[3] {};
	float viewHeight = 0;
	int flags = 0, pmTime = 0, gravity = 800;
	int delta[3] {};
	short stats[PM_STAT_SIZE] {};
	std::vector<Tick> ticks;
};

std::vector<std::string> Split( const std::string &text, char separator ) {
	std::vector<std::string> out;
	std::string item;
	std::istringstream stream( text );
	while( std::getline( stream, item, separator ) ) {
		out.push_back( item );
	}
	if( !text.empty() && text.back() == separator ) {
		out.emplace_back();
	}
	return out;
}

void ReadVec3( const std::string &value, float *out ) {
	std::istringstream stream( value );
	stream >> out[0] >> out[1] >> out[2];
}

bool SetStat( Recording &r, const std::string &name, int value ) {
	static const struct { const char *name; int index; } stats[] = {
		{ "features", PM_STAT_FEATURES }, { "no_user_control", PM_STAT_NOUSERCONTROL },
		{ "knockback", PM_STAT_KNOCKBACK }, { "crouch_time", PM_STAT_CROUCHTIME },
		{ "zoom_time", PM_STAT_ZOOMTIME }, { "dash_time", PM_STAT_DASHTIME },
		{ "wall_jump_time", PM_STAT_WJTIME }, { "no_auto_attack", PM_STAT_NOAUTOATTACK },
		{ "stun", PM_STAT_STUN }, { "max_speed", PM_STAT_MAXSPEED },
		{ "jump_speed", PM_STAT_JUMPSPEED }, { "dash_speed", PM_STAT_DASHSPEED },
		{ "forward_time", PM_STAT_FWDTIME }, { "crouch_slide_time", PM_STAT_CROUCHSLIDETIME },
	};
	for( const auto &stat : stats ) {
		if( name == stat.name ) {
			r.stats[stat.index] = (short)value;
			return true;
		}
	}
	return false;
}

bool ParseRecording( const char *path, Recording &r ) {
	std::ifstream file( path );
	if( !file ) {
		return false;
	}
	std::string line;
	bool inTicks = false;
	while( std::getline( file, line ) ) {
		while( !line.empty() && ( line.back() == '\r' || line.back() == ' ' ) ) {
			line.pop_back();
		}
		if( line.empty() ) {
			continue;
		}
		if( inTicks ) {
			const auto f = Split( line, ',' );
			if( f.size() != 29 ) {
				std::fprintf( stderr, "bad tick row: %s\n", line.c_str() );
				return false;
			}
			Tick t {};
			t.msec = std::atoi( f[1].c_str() );
			t.buttons = std::atoi( f[2].c_str() );
			t.forward = std::atoi( f[3].c_str() );
			t.side = std::atoi( f[4].c_str() );
			t.up = std::atoi( f[5].c_str() );
			for( int i = 0; i < 3; ++i ) {
				t.angles[i] = std::atoi( f[6 + i].c_str() );
				t.origin[i] = std::strtof( f[9 + i].c_str(), nullptr );
				t.velocity[i] = std::strtof( f[12 + i].c_str(), nullptr );
				t.delta[i] = std::atoi( f[20 + i].c_str() );
			}
			t.flags = (int)std::strtoul( f[16].c_str(), nullptr, 10 );
			t.pmTime = std::atoi( f[17].c_str() );
			t.viewHeight = std::strtof( f[18].c_str(), nullptr );
			t.maxsZ = std::strtof( f[19].c_str(), nullptr );
			t.trigger = f[26] == "1";
			r.ticks.push_back( t );
			continue;
		}
		const auto space = line.find( ' ' );
		const std::string key = line.substr( 0, space );
		const std::string value = space == std::string::npos ? "" : line.substr( space + 1 );
		if( key == "start_origin" ) ReadVec3( value, r.origin );
		else if( key == "start_velocity" ) ReadVec3( value, r.velocity );
		else if( key == "start_view_angles" ) ReadVec3( value, r.viewAngles );
		else if( key == "start_view_height" ) r.viewHeight = std::strtof( value.c_str(), nullptr );
		else if( key == "start_flags" ) r.flags = std::atoi( value.c_str() );
		else if( key == "start_pm_time" ) r.pmTime = std::atoi( value.c_str() );
		else if( key == "start_gravity" ) r.gravity = std::atoi( value.c_str() );
		else if( key == "start_delta_angles" ) {
			std::istringstream stream( value );
			stream >> r.delta[0] >> r.delta[1] >> r.delta[2];
		} else if( key.rfind( "stat_", 0 ) == 0 ) {
			SetStat( r, key.substr( 5 ), std::atoi( value.c_str() ) );
		} else if( key == "tick_columns" ) {
			r.columns = value;
			inTicks = true;
		}
		if( !inTicks ) {
			r.header.push_back( key == "generator" ? "generator pmove_replay (legacy gs_pmove.cpp)" : line );
		}
	}
	return true;
}

// ---- Collision hooks ----------------------------------------------------------------------

struct TraceLogEntry {
	bool point;
	int tick;
	std::vector<float> in;
	int mask;
	trace_t trace;
	int contents;
};

cmodel_state_t *cms;
cmodel_s *worldModel;
int currentTick;
int traceFrom = -1, traceTo = -1;
std::vector<TraceLogEntry> traceLog;

struct RecordedEvent {
	int id, parameter;
};
std::vector<RecordedEvent> *activeEvents;

void Trace( trace_t *trace, const vec3_t start, const vec3_t mins, const vec3_t maxs,
			const vec3_t end, int, int mask, int ) {
	CM_TransformedBoxTrace( cms, trace, start, end, mins, maxs, worldModel, mask, vec3_origin, vec3_origin );
	trace->ent = trace->fraction < 1.0f ? 0 : -1;
	if( currentTick >= traceFrom && currentTick <= traceTo ) {
		traceLog.push_back( { false, currentTick,
							  { start[0], start[1], start[2], mins[0], mins[1], mins[2],
								maxs[0], maxs[1], maxs[2], end[0], end[1], end[2] },
							  mask, *trace, 0 } );
	}
}

int PointContents( const vec3_t point, int ) {
	const int contents = CM_TransformedPointContents( cms, point, worldModel, vec3_origin, vec3_origin );
	if( currentTick >= traceFrom && currentTick <= traceTo ) {
		traceLog.push_back( { true, currentTick, { point[0], point[1], point[2] }, 0, {}, contents } );
	}
	return contents;
}

entity_state_t *GetEntityState( int, int ) {
	static entity_state_t worldEntity {};
	worldEntity.type = -1;
	return &worldEntity;
}

void PredictedEvent( int, int event, int parameter ) {
	activeEvents->push_back( { event, parameter } );
}

void TouchTriggers( pmove_t *, const vec3_t ) {}

void WriteTraceLog( const char *path ) {
	std::FILE *out = std::fopen( path, "w" );
	if( !out ) {
		std::fprintf( stderr, "cannot write %s\n", path );
		std::exit( 2 );
	}
	for( const auto &e : traceLog ) {
		if( e.point ) {
			std::fprintf( out, "P %d %.9g %.9g %.9g %d\n", e.tick, e.in[0], e.in[1], e.in[2], e.contents );
			continue;
		}
		const trace_t &t = e.trace;
		std::fprintf( out, "T %d", e.tick );
		for( float v : e.in ) std::fprintf( out, " %.9g", v );
		std::fprintf( out, " %d %.9g %.9g %.9g %.9g %.9g %.9g %.9g %d %d %d %d %d\n", e.mask,
					  t.fraction, t.endpos[0], t.endpos[1], t.endpos[2],
					  t.plane.normal[0], t.plane.normal[1], t.plane.normal[2],
					  t.startsolid ? 1 : 0, t.allsolid ? 1 : 0, t.surfFlags, t.contents, t.ent );
	}
	std::fclose( out );
}

}

int main( int argc, char **argv ) {
	const char *recordingPath = nullptr, *bspPath = nullptr, *outPath = nullptr, *traceOut = nullptr;
	for( int i = 1; i < argc; ++i ) {
		const bool more = i + 1 < argc;
		if( !std::strcmp( argv[i], "--recording" ) && more ) recordingPath = argv[++i];
		else if( !std::strcmp( argv[i], "--bsp" ) && more ) bspPath = argv[++i];
		else if( !std::strcmp( argv[i], "--out" ) && more ) outPath = argv[++i];
		else if( !std::strcmp( argv[i], "--trace-from" ) && more ) traceFrom = std::atoi( argv[++i] );
		else if( !std::strcmp( argv[i], "--trace-to" ) && more ) traceTo = std::atoi( argv[++i] );
		else if( !std::strcmp( argv[i], "--trace-out" ) && more ) traceOut = argv[++i];
		else {
			std::fprintf( stderr, "Usage: %s --recording FILE --bsp FILE --out FILE [--trace-from N --trace-to M --trace-out FILE]\n", argv[0] );
			return 2;
		}
	}
	if( !recordingPath || !bspPath || !outPath ) {
		std::fprintf( stderr, "--recording, --bsp and --out are required\n" );
		return 2;
	}

	Recording recording;
	if( !ParseRecording( recordingPath, recording ) ) {
		std::fprintf( stderr, "cannot read recording %s\n", recordingPath );
		return 2;
	}

	cms = CM_New();
	unsigned checksum = 0;
	worldModel = CM_LoadMap( cms, bspPath, false, &checksum );
	if( !worldModel ) {
		std::fprintf( stderr, "cannot load %s\n", bspPath );
		return 2;
	}

	player_state_t state {};
	state.POVnum = 1;
	state.pmove.pm_type = PM_NORMAL;
	VectorCopy( recording.origin, state.pmove.origin );
	VectorCopy( recording.velocity, state.pmove.velocity );
	state.pmove.pm_flags = recording.flags;
	state.pmove.pm_time = recording.pmTime;
	state.pmove.gravity = recording.gravity;
	std::memcpy( state.pmove.stats, recording.stats, sizeof( recording.stats ) );
	for( int i = 0; i < 3; ++i ) {
		state.pmove.delta_angles[i] = (short)recording.delta[i];
		state.viewangles[i] = recording.viewAngles[i];
	}
	state.viewheight = recording.viewHeight;

	pmove_t move {};
	move.playerState = &state;
	move.snapInitially = false;
	move.contentmask = MASK_PLAYERSOLID;

	gs_state_t gameState {};
	gameState.module = GS_MODULE_CGAME;
	gameState.Trace = Trace;
	gameState.GetEntityState = GetEntityState;
	gameState.PointContents = PointContents;
	gameState.PredictedEvent = PredictedEvent;
	gameState.PMoveTouchTriggers = TouchTriggers;

	std::FILE *out = std::fopen( outPath, "w" );
	if( !out ) {
		std::fprintf( stderr, "cannot write %s\n", outPath );
		return 2;
	}
	for( const auto &line : recording.header ) {
		std::fprintf( out, "%s\n", line.c_str() );
	}
	std::fprintf( out, "tick_columns %s\n", recording.columns.c_str() );

	for( size_t index = 0; index < recording.ticks.size(); ++index ) {
		const Tick &tick = recording.ticks[index];
		currentTick = (int)index;
		move.cmd = {};
		move.cmd.msec = (uint8_t)tick.msec;
		move.cmd.buttons = (uint8_t)tick.buttons;
		move.cmd.forwardmove = (int8_t)tick.forward;
		move.cmd.sidemove = (int8_t)tick.side;
		move.cmd.upmove = (int8_t)tick.up;
		for( int i = 0; i < 3; ++i ) {
			move.cmd.angles[i] = (short)tick.angles[i];
		}

		std::vector<RecordedEvent> events;
		activeEvents = &events;
		Pmove( &gameState, &move );

		// Map entities are not part of the legacy movement code: take the recorded state
		// where triggers (jump pads, teleporters, ...) acted during the original tick.
		if( tick.trigger ) {
			VectorCopy( tick.origin, state.pmove.origin );
			VectorCopy( tick.velocity, state.pmove.velocity );
			state.pmove.pm_flags = tick.flags;
			state.pmove.pm_time = tick.pmTime;
			state.viewheight = tick.viewHeight;
			for( int i = 0; i < 3; ++i ) {
				state.pmove.delta_angles[i] = (short)tick.delta[i];
			}
		}

		std::fprintf( out, "%zu,%d,%d,%d,%d,%d,%d,%d,%d,%.9g,%.9g,%.9g,%.9g,%.9g,%.9g,%d,%d,%d,%.9g,%.9g,%d,%d,%d,%d,%d,%.9g,%d,",
					  index, tick.msec, tick.buttons, tick.forward, tick.side, tick.up,
					  tick.angles[0], tick.angles[1], tick.angles[2],
					  state.pmove.origin[0], state.pmove.origin[1], state.pmove.origin[2],
					  state.pmove.velocity[0], state.pmove.velocity[1], state.pmove.velocity[2],
					  ( state.pmove.pm_flags & PMF_ON_GROUND ) != 0 ? 1 : 0,
					  state.pmove.pm_flags, state.pmove.pm_time, state.viewheight, move.maxs[2],
					  state.pmove.delta_angles[0], state.pmove.delta_angles[1], state.pmove.delta_angles[2],
					  move.waterlevel, move.ladder ? 1 : 0, move.step, tick.trigger ? 1 : 0 );
		for( size_t i = 0; i < events.size(); ++i ) {
			std::fprintf( out, "%s%d:%d", i ? ";" : "", events[i].id, events[i].parameter );
		}
		std::fputc( ',', out );
		for( int i = 0; i < move.numtouch; ++i ) {
			std::fprintf( out, "%s%d", i ? ";" : "", move.touchents[i] );
		}
		std::fputc( '\n', out );
	}
	std::fclose( out );

	if( traceOut ) {
		WriteTraceLog( traceOut );
	}
	return 0;
}
