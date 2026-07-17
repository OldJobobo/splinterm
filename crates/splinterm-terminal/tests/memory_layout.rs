use std::mem::size_of;

use splinterm_terminal::{
    Attributes, Cell, CellContent, Color, ColorSource, Coordinate, CoordinateRange, Cursor, Row,
    ScrollRegion,
};

#[test]
fn phase_one_memory_layout_baseline() {
    let sizes = [
        ("ColorSource", size_of::<ColorSource>()),
        ("Color", size_of::<Color>()),
        ("Attributes", size_of::<Attributes>()),
        ("CellContent", size_of::<CellContent>()),
        ("Cell", size_of::<Cell>()),
        ("Coordinate", size_of::<Coordinate>()),
        ("CoordinateRange", size_of::<CoordinateRange>()),
        ("Cursor", size_of::<Cursor>()),
        ("ScrollRegion", size_of::<ScrollRegion>()),
        ("Row", size_of::<Row>()),
    ];

    for (name, size) in sizes {
        eprintln!("{name:16} {size:>2} bytes");
    }

    assert_eq!(size_of::<ColorSource>(), 1);
    assert_eq!(size_of::<Color>(), 8);
    assert_eq!(size_of::<Attributes>(), 8);
    assert_eq!(size_of::<CellContent>(), 8);
    assert_eq!(size_of::<Cell>(), 12);
    assert_eq!(size_of::<Coordinate>(), 8);
    assert_eq!(size_of::<CoordinateRange>(), 16);
    assert_eq!(size_of::<Cursor>(), 12);
    assert_eq!(size_of::<ScrollRegion>(), 8);
    assert_eq!(size_of::<Row>(), size_of::<usize>() * 4);
}
