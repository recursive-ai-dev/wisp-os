//! Disk selection never depends on which transport enumerated first.

#[derive(Clone, Copy)]
pub struct Candidate {
    pub mounted: bool,
    pub format_allowed: bool,
    pub failed: bool,
}

pub fn automatic_selection(disks: &[Candidate]) -> Option<usize> {
    let mut mounted = disks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.mounted && !d.failed);
    if let Some((index, _)) = mounted.next() {
        return if mounted.next().is_none() {
            Some(index)
        } else {
            None
        };
    }
    let mut usable = disks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.format_allowed && !d.failed);
    let (index, _) = usable.next()?;
    if usable.next().is_none() {
        Some(index)
    } else {
        None
    }
}
