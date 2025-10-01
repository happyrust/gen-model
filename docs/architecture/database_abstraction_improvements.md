# Database Abstraction Layer Improvements

## Overview

This document describes the improvements made to the database abstraction interface to support both TiDB/MySQL and SurrealDB for the gen_geos functionality.

## Architecture

### Core Components

1. **PdmsDataInterface Trait** (`src/data_interface/interface.rs`)
   - Defines the common interface for all database implementations
   - Includes methods for querying elements, attributes, relationships, and geometries
   - Designed to be async-first for better performance

2. **DatabaseInstance Enum** (`src/data_interface/database_factory.rs`)
   - Provides enum-based polymorphism instead of trait objects
   - Supports both TiDB and SurrealDB backends
   ```rust
   pub enum DatabaseInstance {
       TiDB(Arc<AiosDBManager>),
       SurrealDB(Arc<SurrealDBManager>),
   }
   ```

3. **DatabaseFactory** (`src/data_interface/database_factory.rs`)
   - Factory pattern for creating database instances
   - Automatically selects the appropriate backend based on configuration
   - Provides helper methods for common creation patterns

4. **DbOptionExt** (`src/options.rs`)
   - Extended configuration that includes database type selection
   - Supports additional options for remote deployment (MQTT, HTTP servers)
   ```rust
   pub struct DbOptionExt {
       pub inner: DbOption,
       pub db_type: String,  // "tidb", "mysql", or "surrealdb"
       // ... other fields
   }
   ```

### Implementation Details

#### TiDB/MySQL Adapter
- Uses the existing `AiosDBManager` implementation
- Fully compatible with current PDMS data model
- Optimized for relational queries

#### SurrealDB Adapter
- New implementation in `src/data_interface/surreal_manager.rs`
- Provides graph database capabilities
- Currently implements core methods with placeholder implementations for others
- Designed for future expansion with graph-specific optimizations

## Usage

### Configuration

1. **Using Configuration File**
   ```rust
   let db_option_ext = get_db_option_ext();
   let db = DatabaseFactory::create_from_config().await?;
   ```

2. **Programmatic Configuration**
   ```rust
   let db_option_ext = DbOptionExt {
       inner: db_option,
       db_type: "surrealdb".to_string(),
       // ... other options
   };
   let db = DatabaseFactory::create_from_config_ext(&db_option_ext).await?;
   ```

### Working with Database Instances

```rust
// Create database instance
let db = DatabaseFactory::create_from_config().await?;

// Use the database through the common interface
match &db {
    DatabaseInstance::TiDB(manager) => {
        // TiDB-specific operations if needed
    }
    DatabaseInstance::SurrealDB(manager) => {
        // SurrealDB-specific operations if needed
    }
}

// Common operations work regardless of backend
let type_name = db.get_type_name(refno).await;
let attrs = db.get_attr(refno).await?;
```

## Integration with gen_geos

The gen_geos functionality can now work with either database backend:

1. The system automatically selects the appropriate database based on configuration
2. All geometry generation and model processing code remains unchanged
3. Performance characteristics may vary between backends:
   - TiDB: Better for large-scale relational queries
   - SurrealDB: Better for graph traversal and relationship queries

## Benefits

1. **Flexibility**: Switch between databases without code changes
2. **Future-Proof**: Easy to add new database backends
3. **Performance**: Can choose the best database for specific workloads
4. **Compatibility**: Maintains full compatibility with existing code

## Migration Guide

For existing code using `AiosDBManager` directly:

1. Replace direct instantiation with `DatabaseFactory::create_from_config()`
2. Update imports to use the database abstraction types
3. No changes needed to business logic

## Future Enhancements

1. Complete implementation of all methods in SurrealDBManager
2. Add connection pooling for better performance
3. Implement database-specific optimizations
4. Add metrics and monitoring support
5. Support for distributed database deployments

## Examples

See the `examples/` directory for:
- `test_gen_geos.rs`: Basic database abstraction test
- `gen_geos_with_abstraction.rs`: Integration with gen_geos functionality