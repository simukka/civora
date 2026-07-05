use crate::world::VoxelWorld;

/// Result of a voxel raycast.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Hit {
    /// The solid block that was hit.
    pub pos: [i32; 3],
    /// Unit normal of the face that was entered (all zeros if the ray
    /// started inside a solid block).
    pub normal: [i32; 3],
    /// Distance from the origin to the entry point.
    pub distance: f32,
}

/// Walk the voxel grid from `origin` along `dir` (need not be normalized)
/// up to `max_dist`, returning the first solid block.
///
/// Amanatides & Woo DDA traversal: steps exactly one voxel boundary at a
/// time, so thin walls are never skipped.
pub fn raycast(world: &VoxelWorld, origin: [f32; 3], dir: [f32; 3], max_dist: f32) -> Option<Hit> {
    let len = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
    if len <= f32::EPSILON {
        return None;
    }
    let dir = [dir[0] / len, dir[1] / len, dir[2] / len];

    let mut voxel = [
        origin[0].floor() as i32,
        origin[1].floor() as i32,
        origin[2].floor() as i32,
    ];

    if world.get_block(voxel).is_solid() {
        return Some(Hit {
            pos: voxel,
            normal: [0, 0, 0],
            distance: 0.0,
        });
    }

    let mut step = [0i32; 3];
    let mut t_max = [f32::INFINITY; 3];
    let mut t_delta = [f32::INFINITY; 3];
    for axis in 0..3 {
        if dir[axis] > 0.0 {
            step[axis] = 1;
            t_delta[axis] = 1.0 / dir[axis];
            t_max[axis] = ((voxel[axis] as f32 + 1.0) - origin[axis]) / dir[axis];
        } else if dir[axis] < 0.0 {
            step[axis] = -1;
            t_delta[axis] = -1.0 / dir[axis];
            t_max[axis] = (origin[axis] - voxel[axis] as f32) / -dir[axis];
        }
    }

    loop {
        let axis = if t_max[0] < t_max[1] && t_max[0] < t_max[2] {
            0
        } else if t_max[1] < t_max[2] {
            1
        } else {
            2
        };

        let t = t_max[axis];
        if t > max_dist {
            return None;
        }

        voxel[axis] += step[axis];
        t_max[axis] += t_delta[axis];

        if world.get_block(voxel).is_solid() {
            let mut normal = [0i32; 3];
            normal[axis] = -step[axis];
            return Some(Hit {
                pos: voxel,
                normal,
                distance: t,
            });
        }
    }
}
