// The sun's shadow, at half the world's size: from every pixel's depth, a short march toward the
// sun against the depth the camera already drew. Nothing is drawn twice — a phone pays nine depth
// reads a quarter-size pixel instead of a whole extra terrain pass — and what the camera cannot
// see casts nothing, which is the trade every screen-space shadow makes.

override shadows: bool = true;

// Where a camera-relative point lands on the screen: 0..1 from the top left.
fn lite_screen(p: vec3<f32>) -> vec3<f32> {
    let clip = frame.projection * frame.view * vec4<f32>(p, 1.0);
    let w = max(clip.w, 1.0e-4);
    return vec3<f32>(clip.x / w * 0.5 + 0.5, 0.5 - clip.y / w * 0.5, clip.z / w);
}

// 1 in the sun, 0 in a shadow.
fn lite_sun_visibility(depth: f32, uv: vec2<f32>) -> f32 {
    let sun = frame.light_direction;
    if sun.y <= 0.0 {
        return 1.0;
    }
    let origin = view_position(uv, depth);
    var blocked = 0.0;
    var t = 0.75;
    for (var i = 0; i < 9; i++) {
        let q = origin + sun * t;
        let at = lite_screen(q);
        if at.x <= 0.0 || at.x >= 1.0 || at.y <= 0.0 || at.y >= 1.0 {
            break;
        }
        let there = depth_at(at.xy);
        if there > 0.0 {
            let surface = view_position(at.xy, there);
            // A surface nearer the camera than the ray's point stands between it and the sun.
            blocked = max(blocked, smoothstep(0.35, 0.9, length(q) - length(surface)));
        }
        if blocked >= 1.0 {
            break;
        }
        t += 0.75 + t * 0.5;
        if t > 26.0 {
            break;
        }
    }
    return 1.0 - blocked;
}

fn post(input: PostInput) -> vec4<f32> {
    var vis = 1.0;
    if shadows && frame.dimension == 0 && input.depth > 0.0 && frame.sun_direction.y > -0.05 {
        vis = lite_sun_visibility(input.depth, input.uv);
    }
    return vec4<f32>(vis, vis, vis, 1.0);
}
