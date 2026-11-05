#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Camera,
    Screen,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_kind_eq() {
        assert_eq!(SourceKind::Camera, SourceKind::Camera);
        assert_eq!(SourceKind::Screen, SourceKind::Screen);
        assert_ne!(SourceKind::Camera, SourceKind::Screen);
    }

    #[test]
    fn source_kind_copy() {
        let a = SourceKind::Camera;
        let b = a;
        assert_eq!(a, b);
    }
}
