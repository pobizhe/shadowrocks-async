mod error;
mod util;

use error::Error;

pub type Result<T> = std::result::Result<T, Error>;

fn main() {
}
