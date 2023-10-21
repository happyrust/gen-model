use bb8::Pool;
use bb8_arangodb::ArangoConnectionManager;

pub mod create;
pub mod helper;

pub type ArDatabase = arangors_lite::Database;
pub type ArPool = Pool<ArangoConnectionManager>;
