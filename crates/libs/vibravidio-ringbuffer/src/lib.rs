mod inner;
mod reader;
mod writer;

pub use reader::BufferReader;
pub use writer::BufferWriter;

pub fn create_pair<const N: usize, F>() -> (BufferWriter<N, F>, BufferReader<N, F>) {
    let writer = BufferWriter::new();
    let reader = writer.reader();
    (writer, reader)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_pair_returns_linked_pair() {
        let (w, r) = create_pair::<4, u32>();
        w.write_frame(7).unwrap();
        assert_eq!(r.read_frame(), Some(7));
    }
}
