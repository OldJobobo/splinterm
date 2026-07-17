use std::mem::size_of;

use splinterm_terminal::{
    Attributes, Cell, CellContent, Color, ColorSource, Coordinate, CoordinateRange, Cursor, Row,
    ScrollRegion,
};

fn main() {
    for (name, size) in [
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
    ] {
        println!("{name:16} {size:>2} bytes");
    }
}
