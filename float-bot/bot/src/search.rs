// The search: what a try does (Params), how good its outcome is, and picking the next try (random
// at first, then mostly variations of the best ones). Also the line format tries are shared in.
use std::f32::consts::TAU;

/// A try runs at the arch like a player does. It goes `heading` degrees off the start's facing
/// (toward the arch at 0), from `runup` metres back (negative: ahead), `side` metres to the right
/// of that line, looking `yaw` degrees off the start's facing (which side the float drops on
/// depends on it). Full stick in the run-up, `amount` of it while boosting, sprinting or not. The
/// first drop comes when the run passes `first_drop` metres beyond the start. Timings are in game
/// seconds, and the last drop comes as the feet cross the ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub side: f32,
    pub runup: f32,
    pub yaw: f32,
    pub heading: f32,
    pub amount: f32,
    /// Sprinting from 0.5 up.
    pub sprint: f32,
    pub first_drop: f32,
    /// After landing on the float.
    pub jump_after: f32,
    pub grab_after: f32,
    pub next_after: f32,
    /// Feet height above the ceiling that triggers the last drop (negative: below it).
    pub last_drop: f32,
    pub cycles: u32,
}

const N: usize = 12;

/// (min, max) of each Params value in order, cycles last.
pub const RANGES: [(f32, f32); N] = [
    (-0.5, 0.5),
    (-1.0, 5.0),
    (-180.0, 180.0),
    (-100.0, 100.0),
    (0.0, 1.0),
    (0.0, 1.0),
    (-1.5, 1.5),
    (0.0, 0.25),
    (0.0, 0.3),
    (0.02, 0.7),
    (-1.5, 1.5),
    (1.0, 8.0),
];

impl Params {
    pub fn to_array(self) -> [f32; N] {
        [
            self.side,
            self.runup,
            self.yaw,
            self.heading,
            self.amount,
            self.sprint,
            self.first_drop,
            self.jump_after,
            self.grab_after,
            self.next_after,
            self.last_drop,
            self.cycles as f32,
        ]
    }

    pub fn from_array(a: [f32; N]) -> Params {
        let a: [f32; N] = std::array::from_fn(|i| a[i].clamp(RANGES[i].0, RANGES[i].1));
        Params {
            side: a[0],
            runup: a[1],
            yaw: a[2],
            heading: a[3],
            amount: a[4],
            sprint: a[5],
            first_drop: a[6],
            jump_after: a[7],
            grab_after: a[8],
            next_after: a[9],
            last_drop: a[10],
            cycles: a[11].round() as u32,
        }
    }

    pub fn sprinting(&self) -> bool {
        self.sprint >= 0.5
    }

    /// The move stick (right, forward) for going `heading` while looking `yaw`, at `amount`.
    pub fn stick(&self, amount: f32) -> [f32; 2] {
        let (s, c) = (self.heading - self.yaw).to_radians().sin_cos();
        [s * amount, c * amount]
    }

    /// A try of the first bot (which stood still at the start, its stick fixed relative to its
    /// facing, no sprint) in these terms.
    fn from_first_bot(n: &[f32]) -> Params {
        let (right, fwd, yaw, stick_fwd, stick_right) = (n[0], n[1], n[2], n[3], n[4]);
        let heading = yaw + stick_right.atan2(stick_fwd).to_degrees();
        let heading = (heading + 180.0).rem_euclid(360.0) - 180.0;
        let (s, c) = heading.to_radians().sin_cos();
        Params::from_array([
            right * c - fwd * s,
            -(right * s + fwd * c),
            yaw,
            heading,
            stick_fwd.hypot(stick_right),
            0.0,
            RANGES[6].0,
            n[5],
            n[6],
            n[7],
            n[8],
            n[9],
        ])
    }

    pub fn text(&self) -> String {
        format!(
            "elan {:.1} m, decale {:+.2} m, direction {:+.0}, regard {:+.0}, stick {:.1}{}, 1er Q a {:+.1} m, saut {:.0} ms, E {:.0} ms, Q {:.0} ms, dernier Q {:+.1} m, {} tours",
            self.runup,
            self.side,
            self.heading,
            self.yaw,
            self.amount,
            if self.sprinting() { " en sprint" } else { "" },
            self.first_drop,
            self.jump_after * 1000.0,
            self.grab_after * 1000.0,
            self.next_after * 1000.0,
            self.last_drop,
            self.cycles
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FloatEnd {
    InHand,
    /// Its top against the roof above (negative: under it), how far it fell after the drop, and
    /// how high its top ends above the start.
    Landed { top: Option<f32>, fell: f32, rise: f32 },
    /// Left in the roof, sticking out of it, and taken back from up there (what saopy does).
    Regrabbed,
    Lost,
}

/// Why the boosts stopped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum End {
    /// The last drop, at the ceiling.
    LastDrop,
    /// The float was out of reach to take back.
    TooFar,
    /// Every boost done, still under the ceiling.
    Cycles,
    Timeout,
    /// The float wasn't in hand when a drop was due.
    NotHeld,
    /// A try of the first bot, which didn't say.
    Unknown,
}

impl End {
    const ALL: [End; 6] = [End::LastDrop, End::TooFar, End::Cycles, End::Timeout, End::NotHeld, End::Unknown];

    fn code(self) -> usize {
        End::ALL.iter().position(|&e| e == self).unwrap_or(0)
    }

    fn text(self) -> &'static str {
        match self {
            End::LastDrop => "dernier Q au plafond",
            End::TooFar => "bouee hors de portee",
            End::Cycles => "boosts finis sous le plafond",
            End::Timeout => "trop long",
            End::NotHeld => "bouee plus en main",
            End::Unknown => "essai du premier bot",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outcome {
    pub params: Params,
    /// Highest the feet got above the start.
    pub climbed: f32,
    pub on_roof: bool,
    pub float: FloatEnd,
    /// The ceiling above the start.
    pub ceiling: Option<f32>,
    pub boosts: u32,
    pub end: End,
    /// From another game's bot.
    pub shared: bool,
}

impl Outcome {
    /// Higher is better: the player getting up there, then onto the roof, and the float ending up
    /// high, best against the roof or in hand up there.
    pub fn score(&self) -> f32 {
        let h = self.ceiling.unwrap_or(5.0).max(1.0);
        let float = match self.float {
            FloatEnd::InHand | FloatEnd::Regrabbed if self.on_roof => 10.0,
            FloatEnd::Landed { top, fell, rise } => {
                let against = fell <= 1.0 && top.is_some_and(|t| t > -0.3);
                4.0 * (rise / h).clamp(0.0, 1.25) + if against { 2.0 } else { 0.0 }
            },
            _ => 0.0,
        };
        self.climbed.clamp(0.0, h + 1.0) + if self.on_roof { 5.0 } else { 0.0 } + float
    }

    /// On the roof with the float in hand, or lying against the roof just under the player.
    pub fn success(&self) -> bool {
        self.on_roof
            && match self.float {
                FloatEnd::InHand | FloatEnd::Regrabbed => true,
                FloatEnd::Landed { top: Some(t), fell, .. } => fell <= 1.0 && t > -0.3,
                _ => false,
            }
    }

    pub fn summary(&self) -> String {
        let float = match self.float {
            FloatEnd::InHand => "en main".to_string(),
            FloatEnd::Regrabbed => "reprise dans le toit".to_string(),
            FloatEnd::Landed { fell, .. } if fell > 1.0 => format!("tombee de {fell:.1} m"),
            FloatEnd::Landed { top: Some(t), .. } if t > 0.0 => format!("contre le toit (+{:.0} cm)", t * 100.0),
            FloatEnd::Landed { top: Some(t), .. } if t > -1.0 => format!("{:.0} cm sous le toit", -t * 100.0),
            FloatEnd::Landed { rise, .. } => format!("a +{rise:.1} m"),
            FloatEnd::Lost => "perdue".to_string(),
        };
        let ceiling = self.ceiling.map_or(String::new(), |c| format!(" / plafond +{c:.1}"));
        format!(
            "{}{}toi : {} (+{:.1} m{ceiling}) | bouee : {float} | {} boost{}, {}",
            if self.success() { "REUSSI | " } else { "" },
            if self.shared && self.end != End::Unknown { "[autre jeu] " } else { "" },
            if self.on_roof { "sur le toit" } else { "en bas" },
            self.climbed,
            self.boosts,
            if self.boosts > 1 { "s" } else { "" },
            self.end.text()
        )
    }
}

/// Tries before it starts refining its best ones, and how many of those it draws from.
const EXPLORE: usize = 15;
const ELITE: usize = 5;
/// Share of tries still picked at random once refining.
const RANDOM: f32 = 0.25;
/// Size of a variation, as a share of each value's range.
const STEP: f32 = 0.12;

pub struct Search {
    /// Best first.
    pub results: Vec<Outcome>,
    rng: u64,
}

impl Search {
    pub fn new(seed: u64) -> Search {
        Search { results: Vec::new(), rng: seed | 1 }
    }

    fn rand(&mut self) -> f32 {
        // xorshift64
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 40) as f32 / (1u64 << 24) as f32
    }

    fn gauss(&mut self) -> f32 {
        let (u, v) = (self.rand().max(1e-6), self.rand());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }

    /// Random at first and now and then, otherwise a variation of one of the best tries.
    pub fn next_params(&mut self) -> Params {
        if self.results.len() < EXPLORE || self.rand() < RANDOM {
            return Params::from_array(std::array::from_fn(|i| {
                let (lo, hi) = RANGES[i];
                lo + (hi - lo) * self.rand()
            }));
        }
        let elite = self.results.len().min(ELITE);
        let pick = ((self.rand() * elite as f32) as usize).min(elite - 1);
        let mut a = self.results[pick].params.to_array();
        for (i, v) in a.iter_mut().enumerate() {
            let (lo, hi) = RANGES[i];
            *v += self.gauss() * (hi - lo) * STEP;
        }
        Params::from_array(a)
    }

    pub fn add(&mut self, o: Outcome) {
        let at = self.results.iter().position(|r| r.score() < o.score()).unwrap_or(self.results.len());
        self.results.insert(at, o);
        self.results.truncate(500);
    }
}

/// Values after the params in a shared line, before the pid.
const AFTER: usize = 9;

/// A try as a line of the shared file:
/// `R <params> <climbed> <on_roof> <float kind> <top> <fell> <rise> <ceiling> <boosts> <end> <pid>`.
pub fn to_line(o: &Outcome, pid: u32) -> String {
    let (kind, top, fell, rise) = match o.float {
        FloatEnd::InHand => (0, f32::NAN, 0.0, 0.0),
        FloatEnd::Landed { top, fell, rise } => (1, top.unwrap_or(f32::NAN), fell, rise),
        FloatEnd::Lost => (2, f32::NAN, 0.0, 0.0),
        FloatEnd::Regrabbed => (3, f32::NAN, 0.0, 0.0),
    };
    let params: Vec<String> = o.params.to_array().iter().map(|v| format!("{v:.4}")).collect();
    format!(
        "R {} {:.3} {} {kind} {top:.3} {fell:.3} {rise:.3} {:.3} {} {} {pid}",
        params.join(" "),
        o.climbed,
        o.on_roof as i32,
        o.ceiling.unwrap_or(f32::NAN),
        o.boosts,
        o.end.code()
    )
}

/// A line of the first bot: `R <10 params> <climbed> <on_roof> <float kind> <top> <fell> <pid>`.
const FIRST_BOT_FIELDS: usize = 17;

/// A shared line from another process (None for ours, the header, or anything malformed). Tries
/// of the first bot are taken in too, translated.
pub fn from_line(line: &str, my_pid: u32) -> Option<Outcome> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let numbers = |from: usize, to: usize| f[from..to].iter().map(|v| v.parse::<f32>()).collect::<Result<Vec<_>, _>>().ok();
    if f.first() != Some(&"R") || f.last()?.parse::<u32>().ok()? == my_pid {
        return None;
    }
    if f.len() == FIRST_BOT_FIELDS {
        let n = numbers(1, FIRST_BOT_FIELDS - 1)?;
        return Some(Outcome {
            params: Params::from_first_bot(&n),
            climbed: n[10],
            on_roof: n[11] != 0.0,
            float: match n[12] as i32 {
                0 => FloatEnd::InHand,
                1 => FloatEnd::Landed { top: n[13].is_finite().then_some(n[13]), fell: n[14], rise: 0.0 },
                _ => FloatEnd::Lost,
            },
            ceiling: None,
            boosts: n[9] as u32,
            end: End::Unknown,
            shared: true,
        });
    }
    if f.len() != 1 + N + AFTER + 1 {
        return None;
    }
    let n = numbers(1, 1 + N + AFTER)?;
    let finite = |v: f32| v.is_finite().then_some(v);
    let float = match n[N + 2] as i32 {
        0 => FloatEnd::InHand,
        1 => FloatEnd::Landed { top: finite(n[N + 3]), fell: n[N + 4], rise: n[N + 5] },
        3 => FloatEnd::Regrabbed,
        _ => FloatEnd::Lost,
    };
    Some(Outcome {
        params: Params::from_array(n[..N].try_into().ok()?),
        climbed: n[N],
        on_roof: n[N + 1] != 0.0,
        float,
        ceiling: finite(n[N + 6]),
        boosts: n[N + 7] as u32,
        end: *End::ALL.get(n[N + 8] as usize)?,
        shared: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> Params {
        Params::from_array([0.1, 2.0, 15.0, -20.0, 0.8, 1.0, 0.5, 0.1, 0.05, 0.2, -0.5, 3.0])
    }

    fn outcome(on_roof: bool, float: FloatEnd) -> Outcome {
        Outcome { params: params(), climbed: 2.0, on_roof, float, ceiling: Some(4.5), boosts: 3, end: End::LastDrop, shared: false }
    }

    fn landed(top: f32, fell: f32, rise: f32) -> FloatEnd {
        FloatEnd::Landed { top: Some(top), fell, rise }
    }

    #[test]
    fn params_stay_in_range() {
        let wild = Params::from_array([9.0, -9.0, 400.0, 300.0, 3.0, -3.0, 3.0, -1.0, 5.0, 0.0, 9.0, 99.0]);
        assert_eq!(wild.to_array(), [0.5, -1.0, 180.0, 100.0, 1.0, 0.0, 1.5, 0.0, 0.3, 0.02, 1.5, 8.0]);
        assert_eq!(Params::from_array(params().to_array()), params());
        let mut search = Search::new(42);
        for i in 0..300 {
            if i == 20 {
                // Enough results to start refining.
                for _ in 0..EXPLORE {
                    search.add(outcome(true, FloatEnd::Lost));
                }
            }
            let p = search.next_params().to_array();
            assert!(p.iter().zip(RANGES).all(|(v, (lo, hi))| (lo..=hi).contains(v)));
        }
    }

    #[test]
    fn best_tries_first() {
        let in_hand = outcome(true, FloatEnd::InHand);
        let against = outcome(true, landed(0.05, 0.2, 4.6));
        let fell = outcome(true, landed(-4.0, 2.5, 0.4));
        let below = outcome(false, FloatEnd::InHand);
        assert!(in_hand.success() && against.success() && !fell.success() && !below.success());
        assert!(in_hand.score() > against.score() && against.score() > fell.score() && fell.score() > below.score());
        let mut search = Search::new(1);
        for o in [below, fell, in_hand, against] {
            search.add(o);
        }
        assert_eq!(search.results, [in_hand, against, fell, below]);
        assert_eq!(
            against.summary(),
            "REUSSI | toi : sur le toit (+2.0 m / plafond +4.5) | bouee : contre le toit (+5 cm) | 3 boosts, dernier Q au plafond"
        );
    }

    #[test]
    fn a_float_on_the_floor_counts_for_nothing() {
        // Lying where it was dropped at the start, whether it fell or not: only the climb counts.
        let higher = Outcome { climbed: 3.5, ..outcome(false, landed(-4.6, 1.7, 0.3)) };
        let lower = Outcome { climbed: 2.0, ..outcome(false, landed(-4.6, 0.3, 0.3)) };
        assert!(higher.score() > lower.score());
        // A float left high up is worth more than a bit more climb.
        let high_float = Outcome { climbed: 3.0, ..outcome(false, landed(-0.8, 1.5, 3.9)) };
        assert!(high_float.score() > higher.score());
    }

    #[test]
    fn first_bot_tries_translate() {
        // Turned -157, stick mostly right: it went about -85 off the start's facing, standing
        // 0.34 m left of the start and 0.26 m ahead, without sprinting, dropping at once.
        let line = "R -0.3400 0.2600 -156.7600 0.2900 0.8600 0.0600 0.1100 0.4800 0.3000 4.0000 7.370 1 1 -5.330 5.160 203392";
        let o = from_line(line, 1).unwrap();
        let p = o.params;
        assert!((p.heading + 85.4).abs() < 0.5 && (p.yaw + 156.76).abs() < 1e-3, "{p:?}");
        assert!((p.amount - 0.9076).abs() < 1e-3 && !p.sprinting() && p.first_drop == RANGES[6].0);
        // Same spot: back along the heading by -runup, then side to its right.
        let (s, c) = p.heading.to_radians().sin_cos();
        let (right, fwd) = (-p.runup * s + p.side * c, -p.runup * c - p.side * s);
        assert!((right + 0.34).abs() < 1e-3 && (fwd - 0.26).abs() < 1e-3, "{right} {fwd}");
        // Same stick as the first bot held, relative to its facing.
        let [x, y] = p.stick(p.amount);
        assert!((x - 0.86).abs() < 1e-3 && (y - 0.29).abs() < 1e-3, "{x} {y}");
        assert!(o.on_roof && o.boosts == 4 && (p.last_drop - 0.3).abs() < 1e-3 && p.cycles == 4);
        assert!(o.summary().starts_with("toi : sur le toit (+7.4 m) | bouee : tombee de 5.2 m"), "{}", o.summary());
    }

    #[test]
    fn refines_around_the_best() {
        let mut search = Search::new(7);
        let best = Outcome { climbed: 3.0, ..outcome(true, FloatEnd::InHand) };
        search.add(best);
        for _ in 0..EXPLORE {
            search.add(outcome(false, FloatEnd::Lost));
        }
        // Most picks are small steps away from one of the best tries.
        let near = (0..200)
            .filter(|_| {
                let p = search.next_params();
                (p.yaw - params().yaw).abs() < 90.0 && (p.runup - params().runup).abs() < 1.5
            })
            .count();
        assert!(near > 120, "{near}");
    }

    #[test]
    fn shared_lines_round_trip() {
        let o = Outcome { end: End::TooFar, ..outcome(true, landed(-0.25, 0.4, 4.2)) };
        let line = to_line(&o, 1234);
        let back = from_line(&line, 99).unwrap();
        assert!(back.shared && back.on_roof && (back.climbed - 2.0).abs() < 1e-3);
        assert!(matches!(back.float, FloatEnd::Landed { top: Some(t), fell, rise }
            if (t + 0.25).abs() < 1e-3 && (fell - 0.4).abs() < 1e-3 && (rise - 4.2).abs() < 1e-3));
        assert!((back.params.yaw - 15.0).abs() < 1e-3 && back.params.cycles == 3);
        assert!((back.params.runup - 2.0).abs() < 1e-3 && (back.params.heading + 20.0).abs() < 1e-3 && back.params.sprinting());
        assert_eq!((back.ceiling, back.boosts, back.end), (Some(4.5), 3, End::TooFar));
        assert_eq!(from_line(&line, 1234), None); // our own
        assert_eq!(from_line("START 1 2 3 4 Unicorn", 99), None);
        assert_eq!(from_line("R 1 2 3", 99), None);
        let lost = from_line(&to_line(&Outcome { ceiling: None, ..outcome(false, FloatEnd::Lost) }, 5), 6).unwrap();
        assert_eq!((lost.float, lost.ceiling), (FloatEnd::Lost, None));
        let regrabbed = from_line(&to_line(&outcome(true, FloatEnd::Regrabbed), 5), 6).unwrap();
        assert!(regrabbed.float == FloatEnd::Regrabbed && regrabbed.success());
    }
}
