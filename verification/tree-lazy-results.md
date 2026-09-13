# AMS tree lazy read

Current-source command: cargo test --lib ams_tree_first_read -- --ignored --nocapture
Exit status 0. 1 passed.
AMS roots=38, ancestors=5, database opens=63, scans=0, scanned elements=0, point lookups=580.
Literal output: tree-lazy-ams.log.

DirectTreeService no longer loads MdbMemoryStore. It freezes per-database sessions from headers, gets WORLD and extent refs from descriptor/file_info, and reads OWNER/TYPE/members on demand through DirectStore. /dbnums and search response metadata use TreeMetadata. The LoadedDb value type is still reused; elements=0 means no census performed.

Limitations: this is a file-backed tree test, not HTTP/UI or SAVEWORK/restart acceptance. Health/increment_manager skeleton remnants and the two previously reported full-suite failures remain to be addressed. Existing baseline test output is not proof of full E3D geometry parity.
