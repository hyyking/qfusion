#include <algorithm>
#include <array>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <limits>
#include <vector>

#include <common/facilities/gs_public.h>

void Sys_Error( const char *, ... ) {
	std::abort();
}

namespace {

struct CollisionPlane {
	std::array<float, 3> normal;
	float distance;
	int surfaceFlags;
	int entity;
};

struct CollisionBox {
	std::array<float, 3> mins;
	std::array<float, 3> maxs;
	int surfaceFlags;
	int entity;
};

struct World {
	std::vector<CollisionPlane> planes;
	std::vector<CollisionBox> boxes;
	bool water;
};

struct RecordedEvent {
	int id;
	int parameter;
};

World activeWorld;
std::vector<RecordedEvent> *activeEvents = nullptr;

void Trace( trace_t *trace, const vec3_t start, const vec3_t mins, const vec3_t maxs,
			const vec3_t end, int, int, int ) {
	std::memset( trace, 0, sizeof( *trace ) );
	trace->fraction = 1.0f;
	trace->ent = -1;
	VectorCopy( end, trace->endpos );

	for( const auto &plane : activeWorld.planes ) {
		float support = 0.0f;
		float startDistance = -plane.distance;
		float endDistance = -plane.distance;
		for( int axis = 0; axis < 3; ++axis ) {
			support += plane.normal[axis] * ( plane.normal[axis] > 0.0f ? mins[axis] : maxs[axis] );
			startDistance += plane.normal[axis] * start[axis];
			endDistance += plane.normal[axis] * end[axis];
		}
		startDistance += support;
		endDistance += support;

		if( startDistance < 0.0f ) {
			trace->startsolid = true;
			trace->allsolid = endDistance < 0.0f;
			continue;
		}
		if( endDistance >= 0.0f || endDistance == startDistance ) {
			continue;
		}

		const float fraction = startDistance / ( startDistance - endDistance );
		if( fraction >= trace->fraction ) {
			continue;
		}

		trace->fraction = fraction;
		vec3_t delta;
		VectorSubtract( end, start, delta );
		VectorMA( start, fraction, delta, trace->endpos );
		VectorCopy( plane.normal.data(), trace->plane.normal );
		trace->plane.dist = plane.distance;
		trace->surfFlags = plane.surfaceFlags;
		trace->contents = CONTENTS_SOLID;
		trace->ent = plane.entity;
		trace->allsolid = false;
	}

	for( const auto &box : activeWorld.boxes ) {
		std::array<float, 3> expandedMins;
		std::array<float, 3> expandedMaxs;
		bool startInside = true;
		bool endInside = true;
		float enter = -std::numeric_limits<float>::infinity();
		float leave = 1.0f;
		int hitAxis = -1;
		float hitNormal = 0.0f;
		bool intersects = true;

		for( int axis = 0; axis < 3; ++axis ) {
			expandedMins[axis] = box.mins[axis] - maxs[axis];
			expandedMaxs[axis] = box.maxs[axis] - mins[axis];
			startInside = startInside && start[axis] > expandedMins[axis] && start[axis] < expandedMaxs[axis];
			endInside = endInside && end[axis] > expandedMins[axis] && end[axis] < expandedMaxs[axis];

			const float delta = end[axis] - start[axis];
			if( delta == 0.0f ) {
				if( start[axis] <= expandedMins[axis] || start[axis] >= expandedMaxs[axis] ) {
					intersects = false;
				}
				continue;
			}

			float nearDistance = ( expandedMins[axis] - start[axis] ) / delta;
			float farDistance = ( expandedMaxs[axis] - start[axis] ) / delta;
			float normal = -1.0f;
			if( nearDistance > farDistance ) {
				std::swap( nearDistance, farDistance );
				normal = 1.0f;
			}
			if( nearDistance > enter ) {
				enter = nearDistance;
				hitAxis = axis;
				hitNormal = normal;
			}
			leave = std::min( leave, farDistance );
			if( enter > leave ) {
				intersects = false;
			}
		}

		if( startInside ) {
			trace->startsolid = true;
			trace->allsolid = endInside;
			continue;
		}
		if( !intersects || hitAxis < 0 || enter < 0.0f || enter >= trace->fraction || enter > leave ) {
			continue;
		}

		trace->fraction = enter;
		vec3_t delta;
		VectorSubtract( end, start, delta );
		VectorMA( start, enter, delta, trace->endpos );
		VectorClear( trace->plane.normal );
		trace->plane.normal[hitAxis] = hitNormal;
		trace->plane.dist = hitNormal > 0.0f ? box.maxs[hitAxis] : -box.mins[hitAxis];
		trace->surfFlags = box.surfaceFlags;
		trace->contents = CONTENTS_SOLID;
		trace->ent = box.entity;
		trace->allsolid = false;
	}
}

int PointContents( const vec3_t point, int ) {
	return activeWorld.water && point[2] >= 0.0f && point[2] <= 256.0f ? CONTENTS_WATER : 0;
}

entity_state_t *GetEntityState( int, int ) {
	static entity_state_t worldEntity {};
	worldEntity.type = -1;
	return &worldEntity;
}

void PredictedEvent( int, int event, int parameter ) {
	activeEvents->push_back( { event, parameter } );
}

void TouchTriggers( pmove_t *, const vec3_t ) {
}

struct Case {
	const char *name;
	int duration;
	float startX;
	float startZ;
	float startVelocityX;
	float startVelocityZ;
	bool wall;
	bool water;
	bool ladder;
	enum class Geometry {
		Flat,
		Slope,
		Step,
		Stairs,
		BlockedStand
	} geometry = Geometry::Flat;
};

constexpr Case cases[] = {
	{ "jump_tap", 1600, 0, 24, 0, 0, false, false, false },
	{ "jump_hold", 1600, 0, 24, 0, 0, false, false, false },
	{ "air_direction_change", 1600, 0, 24, 0, 0, false, false, false },
	{ "dash_wall_hit", 1600, 0, 24, 0, 0, true, false, false },
	{ "walljump_after_dash", 1600, 0, 24, 0, 0, true, false, false },
	{ "land_slow", 1600, 0, 180, 90, -100, false, false, false },
	{ "land_medium", 1600, 0, 180, 220, -300, false, false, false },
	{ "land_fast", 1600, 0, 180, 360, -500, false, false, false },
	{ "water_swim", 1600, 0, 24, 0, 0, false, true, false },
	{ "ladder_climb", 1600, 94, 40, 0, 0, true, false, true },
	{ "slope_walk", 1600, 0, 24, 0, 0, false, false, false, Case::Geometry::Slope },
	{ "step_up", 1600, 0, 24, 0, 0, false, false, false, Case::Geometry::Step },
	{ "stairs_walk", 1600, 0, 24, 0, 0, false, false, false, Case::Geometry::Stairs },
	{ "crouch_slide", 1600, 0, 120, 450, -200, false, false, false },
	{ "crouch_uncrouch", 1600, 0, 24, 0, 0, false, false, false },
	{ "crouch_blocked_stand", 1600, 0, 24, 0, 0, false, false, false, Case::Geometry::BlockedStand }
};

int FrameMsec( double rate ) {
	if( rate == 125 ) return 8;
	if( rate == 100 ) return 10;
	if( rate == 62.5 ) return 16;
	if( rate == 60 ) return 17;
	if( rate == 50 ) return 20;
	if( rate == 30 ) return 33;
	return 0;
}

const char *EventName( int event ) {
	switch( event ) {
		case EV_JUMP: return "JUMP";
		case EV_DOUBLEJUMP: return "DOUBLEJUMP";
		case EV_DASH: return "DASH";
		case EV_WALLJUMP: return "WALLJUMP";
		case EV_WALLJUMP_FAILED: return "WALLJUMP_FAILED";
		case EV_FALL: return "FALL";
		default: return "OTHER";
	}
}

void EmitCase( const Case &testCase, double rate ) {
	const int msec = FrameMsec( rate );
	activeWorld = {};
	if( testCase.geometry == Case::Geometry::Slope ) {
		activeWorld.planes.push_back( { { -0.24253562f, 0.0f, 0.9701425f }, 0.0f, 0, 1 } );
	} else {
		activeWorld.planes.push_back( { { 0.0f, 0.0f, 1.0f }, 0.0f, 0, 1 } );
	}
	if( testCase.geometry == Case::Geometry::Step ) {
		activeWorld.boxes.push_back( { { 48.0f, -128.0f, 0.0f }, { 80.0f, 128.0f, 12.0f }, 0, 3 } );
	} else if( testCase.geometry == Case::Geometry::Stairs ) {
		activeWorld.boxes.push_back( { { 48.0f, -128.0f, 0.0f }, { 72.0f, 128.0f, 6.0f }, 0, 3 } );
		activeWorld.boxes.push_back( { { 72.0f, -128.0f, 0.0f }, { 96.0f, 128.0f, 12.0f }, 0, 4 } );
		activeWorld.boxes.push_back( { { 96.0f, -128.0f, 0.0f }, { 120.0f, 128.0f, 18.0f }, 0, 5 } );
	} else if( testCase.geometry == Case::Geometry::BlockedStand ) {
		activeWorld.boxes.push_back( { { -32.0f, -64.0f, 40.0f }, { 48.0f, 64.0f, 80.0f }, 0, 3 } );
	}
	activeWorld.water = testCase.water;
	if( testCase.wall ) {
		activeWorld.planes.push_back( { { -1.0f, 0.0f, 0.0f }, -110.0f, testCase.ladder ? SURF_LADDER : 0, 2 } );
	}

	player_state_t state {};
	state.POVnum = 1;
	state.pmove.pm_type = PM_NORMAL;
	state.pmove.origin[0] = testCase.startX;
	state.pmove.origin[2] = testCase.startZ;
	state.pmove.velocity[0] = testCase.startVelocityX;
	state.pmove.velocity[2] = testCase.startVelocityZ;
	state.pmove.gravity = GRAVITY;
	state.pmove.stats[PM_STAT_FEATURES] = PMFEAT_JUMP | PMFEAT_DASH | PMFEAT_WALLJUMP |
		PMFEAT_AIRCONTROL | PMFEAT_FWDBUNNY | PMFEAT_CROUCH | PMFEAT_CROUCHSLIDING;
	state.pmove.stats[PM_STAT_MAXSPEED] = DEFAULT_PLAYERSPEED_STANDARD;
	state.pmove.stats[PM_STAT_JUMPSPEED] = DEFAULT_JUMPSPEED;
	state.pmove.stats[PM_STAT_DASHSPEED] = DEFAULT_DASHSPEED;

	pmove_t move {};
	move.playerState = &state;
	move.snapInitially = false;
	move.contentmask = MASK_PLAYERSOLID;

	for( int elapsed = 0, step = 0; elapsed < testCase.duration; elapsed += msec, ++step ) {
		move.cmd = {};
		move.cmd.msec = static_cast<uint8_t>( msec );
		if( std::strcmp( testCase.name, "jump_tap" ) == 0 ) {
			move.cmd.upmove = elapsed < msec || ( elapsed >= 900 && elapsed < 900 + msec ) ? 127 : 0;
		} else if( std::strcmp( testCase.name, "jump_hold" ) == 0 ) {
			move.cmd.upmove = elapsed < 1200 ? 127 : 0;
		} else if( std::strcmp( testCase.name, "air_direction_change" ) == 0 ) {
			move.cmd.upmove = elapsed < msec ? 127 : 0;
			move.cmd.forwardmove = elapsed < 400 ? 127 : 0;
			move.cmd.sidemove = elapsed >= 400 && elapsed < 1000 ? 127 : 0;
		} else if( std::strcmp( testCase.name, "dash_wall_hit" ) == 0 ) {
			move.cmd.forwardmove = 127;
			move.cmd.buttons = elapsed < msec ? BUTTON_SPECIAL : 0;
		} else if( std::strcmp( testCase.name, "walljump_after_dash" ) == 0 ) {
			move.cmd.forwardmove = 127;
			move.cmd.buttons = ( elapsed < msec || ( elapsed >= 240 && elapsed < 240 + msec ) ) ? BUTTON_SPECIAL : 0;
		} else if( std::strcmp( testCase.name, "water_swim" ) == 0 ) {
			move.cmd.forwardmove = elapsed < 800 ? 127 : 0;
			move.cmd.upmove = elapsed < 400 ? 127 : 0;
		} else if( std::strcmp( testCase.name, "ladder_climb" ) == 0 ) {
			move.cmd.upmove = elapsed < 800 ? 127 : 0;
		} else if( std::strcmp( testCase.name, "slope_walk" ) == 0 ||
				   std::strcmp( testCase.name, "step_up" ) == 0 ||
				   std::strcmp( testCase.name, "stairs_walk" ) == 0 ) {
			move.cmd.forwardmove = 127;
		} else if( std::strcmp( testCase.name, "crouch_slide" ) == 0 ) {
			move.cmd.upmove = elapsed < 600 ? -127 : 0;
		} else if( std::strcmp( testCase.name, "crouch_uncrouch" ) == 0 ) {
			move.cmd.upmove = elapsed < 300 ? -127 : 0;
		} else if( std::strcmp( testCase.name, "crouch_blocked_stand" ) == 0 ) {
			move.cmd.upmove = elapsed < 160 ? -127 : 0;
			move.cmd.forwardmove = elapsed >= 160 ? 127 : 0;
		}

		std::vector<RecordedEvent> events;
		activeEvents = &events;
		gs_state_t gameState {};
		gameState.module = GS_MODULE_CGAME;
		gameState.Trace = Trace;
		gameState.GetEntityState = GetEntityState;
		gameState.PointContents = PointContents;
		gameState.PredictedEvent = PredictedEvent;
		gameState.PMoveTouchTriggers = TouchTriggers;
		Pmove( &gameState, &move );

		std::printf( "%s,%g,%d,%d,%d,%u,%d,%d,%d,%d,%d,%d,%.6f,%.6f,%.6f,%.6f,%.6f,%.6f,%d,%d,%d,0x%08x,\"",
					 testCase.name, rate, step, elapsed, msec, move.cmd.buttons,
					 move.cmd.forwardmove, move.cmd.sidemove, move.cmd.upmove,
					 move.cmd.angles[0], move.cmd.angles[1], move.cmd.angles[2],
					 state.pmove.origin[0], state.pmove.origin[1], state.pmove.origin[2],
					 state.pmove.velocity[0], state.pmove.velocity[1], state.pmove.velocity[2],
					 ( state.pmove.pm_flags & PMF_ON_GROUND ) != 0,
					 move.waterlevel, move.ladder,
					 static_cast<unsigned>( state.pmove.pm_flags ) );
		for( size_t i = 0; i < events.size(); ++i ) {
			if( i ) std::putchar( ';' );
			std::printf( "%s[%d]:%d", EventName( events[i].id ), events[i].id, events[i].parameter );
		}
		std::printf( "\",\"" );
		for( int i = 0; i < move.numtouch; ++i ) {
			if( i ) std::putchar( ';' );
			std::printf( "%d", move.touchents[i] );
		}
		std::printf( "\",%.6f,%.6f\n", move.step, state.viewheight );
	}
}

}

int main( int argc, char **argv ) {
	double selectedRate = 0;
	const char *selectedCase = nullptr;
	for( int i = 1; i < argc; ++i ) {
		if( std::strcmp( argv[i], "--rate" ) == 0 && i + 1 < argc ) {
			selectedRate = std::atof( argv[++i] );
			if( !FrameMsec( selectedRate ) ) {
				std::fprintf( stderr, "Unsupported rate. Choose 125, 100, 62.5, 60, 50, or 30.\n" );
				return 2;
			}
		} else if( std::strcmp( argv[i], "--case" ) == 0 && i + 1 < argc ) {
			selectedCase = argv[++i];
		} else {
			std::fprintf( stderr, "Usage: %s [--rate HZ] [--case NAME]\n", argv[0] );
			return 2;
		}
	}

	const double rates[] = { 125, 100, 62.5, 60, 50, 30 };
	std::puts( "scenario,rate_hz,step,time_ms,msec,buttons,forwardmove,sidemove,upmove,pitch,yaw,roll,x,y,z,vx,vy,vz,grounded,waterlevel,ladder,flags,events,touches,step_height,view_height" );
	bool matchedCase = !selectedCase;
	for( const auto &testCase : cases ) {
		if( selectedCase && std::strcmp( selectedCase, testCase.name ) != 0 ) {
			continue;
		}
		matchedCase = true;
		for( const auto rate : rates ) {
			if( !selectedRate || selectedRate == rate ) {
				EmitCase( testCase, rate );
			}
		}
	}
	if( !matchedCase ) {
		std::fprintf( stderr, "Unknown case: %s\n", selectedCase );
		return 2;
	}
	return 0;
}
