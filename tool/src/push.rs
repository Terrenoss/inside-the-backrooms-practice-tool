// Inspect glitch push: the pure part of the measurement (player frame, how a move splits into back /
// up / side, the wall behind, the verdict). game.rs does the reading and the drawing.
use crate::game::V3;

/// The player's horizontal frame, from the camera's right vector: unlike its forward one, it stays
/// horizontal and defined when looking straight down (the inspect glitch is done looking at the floor).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    pub right: V3,
    pub fwd: V3,
}

impl Frame {
    pub(crate) fn from_camera_right(right: V3) -> Frame {
        let flat = V3 { y: 0.0, ..right };
        let right = if flat.len() < 1e-4 { V3 { x: 1.0, y: 0.0, z: 0.0 } } else { flat.scale(1.0 / flat.len()) };
        Frame { right, fwd: V3 { x: -right.z, y: 0.0, z: right.x } }
    }

    pub(crate) fn split(&self, d: V3) -> Split {
        Split { back: -d.dot(self.fwd), up: d.y, side: d.dot(self.right) }
    }

    /// Facing on the map, degrees (atan2(x, z) of forward, like Unity's yaw).
    pub(crate) fn yaw_deg(&self) -> f32 {
        self.fwd.x.atan2(self.fwd.z).to_degrees()
    }
}

/// A move in the player's frame: back > 0 is backward, up > 0 upward, side > 0 to the right.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Split {
    pub back: f32,
    pub up: f32,
    pub side: f32,
}

impl Split {
    pub(crate) fn len(&self) -> f32 {
        (self.back * self.back + self.up * self.up + self.side * self.side).sqrt()
    }
}

/// How far the camera looks below the horizon, degrees (90 = straight at the floor).
pub(crate) fn pitch_down_deg(cam_fwd: V3) -> f32 {
    (-cam_fwd.y).clamp(-1.0, 1.0).asin().to_degrees()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pose {
    Debout,
    Accroupi,
    Allonge,
    Inconnue,
}

impl Pose {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Pose::Debout => "debout",
            Pose::Accroupi => "accroupi",
            Pose::Allonge => "allonge",
            Pose::Inconnue => "?",
        }
    }
}

/// The wall behind the player when the inspection opened: distances backward from the capsule's
/// center to its near and far faces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Wall {
    pub near: f32,
    pub far: f32,
}

impl Wall {
    pub(crate) fn thickness(&self) -> f32 {
        self.far - self.near
    }
}

/// `near`: first hit going backward. `back_hit`: first hit coming back from `near + probe` toward
/// the player (the far face). No far face within the probe: the wall is at least `probe` thick.
pub(crate) fn wall_from_hits(near: Option<f32>, back_hit: Option<f32>, probe: f32) -> Option<Wall> {
    let near = near?;
    Some(Wall { near, far: near + probe - back_hit.unwrap_or(0.0) })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Verdict {
    Traverse,
    Reste,
    Recrache,
    Monte,
    Rien,
    PasDeMur,
}

impl Verdict {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Verdict::Traverse => "A TRAVERSE",
            Verdict::Reste => "reste en arriere",
            Verdict::Recrache => "recrache",
            Verdict::Monte => "te fait monter",
            Verdict::Rien => "pas pousse",
            Verdict::PasDeMur => "pas de mur derriere",
        }
    }

    pub(crate) fn color(self) -> [f32; 4] {
        match self {
            Verdict::Traverse => [0.2, 1.0, 0.3, 1.0],
            Verdict::Reste => [1.0, 0.85, 0.2, 1.0],
            Verdict::Recrache => [1.0, 0.6, 0.1, 1.0],
            Verdict::Monte => [1.0, 0.25, 0.25, 1.0],
            Verdict::Rien => [0.7, 0.7, 0.7, 1.0],
            Verdict::PasDeMur => [0.85, 0.85, 0.85, 1.0],
        }
    }
}

/// `open`: the push in the first moments; `settled`: where the player is once it played out (the
/// wall may have thrown them back). Pushes of 0.27 m or more stayed in the logs, smaller ones came back.
pub(crate) fn verdict(open: Split, settled: Split, wall: Option<&Wall>, crossed: bool) -> Verdict {
    if crossed || wall.is_some_and(|w| settled.back > w.far) {
        Verdict::Traverse
    } else if open.up > 0.1 && open.up > open.back {
        Verdict::Monte
    } else if settled.back >= 0.1 {
        if wall.is_some() { Verdict::Reste } else { Verdict::PasDeMur }
    } else if open.back >= 0.05 || open.len() >= 0.05 {
        if wall.is_some() { Verdict::Recrache } else { Verdict::PasDeMur }
    } else {
        Verdict::Rien
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_from_right_survives_looking_down() {
        // regard droit vers le sol, tourne vers +X : la droite de la camera vaut (0,0,-1)
        let f = Frame::from_camera_right(V3 { x: 0.0, y: 0.0, z: -1.0 });
        assert!(f.fwd.dist(V3 { x: 1.0, y: 0.0, z: 0.0 }) < 1e-5);
        assert!((f.yaw_deg() - 90.0).abs() < 1e-3);
        let s = f.split(V3 { x: -0.27, y: 0.0, z: 0.0 });
        assert!((s.back - 0.27).abs() < 1e-5 && s.up.abs() < 1e-6 && s.side.abs() < 1e-6);
    }

    #[test]
    fn pitch_convention() {
        assert!((pitch_down_deg(V3 { x: 0.0, y: -1.0, z: 0.0 }) - 90.0).abs() < 1e-3);
        let f = V3 { x: 0.0, y: -(62f32.to_radians().sin()), z: 62f32.to_radians().cos() };
        assert!((pitch_down_deg(f) - 62.0).abs() < 1e-3);
    }

    #[test]
    fn wall_geometry() {
        let w = wall_from_hits(Some(0.3), Some(2.6), 3.0).unwrap();
        assert!((w.far - 0.7).abs() < 1e-5 && (w.thickness() - 0.4).abs() < 1e-5);
        assert!(wall_from_hits(None, Some(1.0), 3.0).is_none());
        assert!((wall_from_hits(Some(0.3), None, 3.0).unwrap().far - 3.3).abs() < 1e-5);
    }

    #[test]
    fn verdict_cases_from_the_logs() {
        let sp = |back, up| Split { back, up, side: 0.0 };
        let wall = Wall { near: 0.3, far: 0.9 };
        // mur 2 a 62 deg : recul 0,27 qui reste
        assert_eq!(verdict(sp(0.27, 0.0), sp(0.27, 0.0), Some(&wall), false), Verdict::Reste);
        // mur 2 a 76-80 deg : monte de 0,27
        assert_eq!(verdict(sp(0.0, 0.27), sp(0.0, 0.0), Some(&wall), false), Verdict::Monte);
        // mur 2 a 47 deg : recul 0,25 puis recrache
        assert_eq!(verdict(sp(0.25, 0.0), sp(0.0, 0.0), Some(&wall), false), Verdict::Recrache);
        // passe la face arriere
        assert_eq!(verdict(sp(0.37, 0.0), sp(1.0, 0.0), Some(&wall), false), Verdict::Traverse);
        assert_eq!(verdict(sp(0.0, 0.0), sp(0.0, 0.0), Some(&wall), false), Verdict::Rien);
    }

    #[test]
    fn verdict_without_wall() {
        let sp = |back| Split { back, up: 0.0, side: 0.0 };
        assert_eq!(verdict(sp(0.3), sp(0.3), None, false), Verdict::PasDeMur);
        assert_eq!(verdict(sp(0.3), sp(0.3), None, true), Verdict::Traverse);
    }
}
