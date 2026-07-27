pub trait Compress<T> {
    fn compress(&self) -> T;
}

pub trait Decompress<T> {
    fn decompress(&self) -> T;
}
