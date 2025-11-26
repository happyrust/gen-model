DEFINE TABLE type_node SCHEMAFULL
    PERMISSIONS
        FULL;

DEFINE FIELD code ON type_node TYPE string
    ASSERT $value != NONE;

DEFINE FIELD hash ON type_node TYPE option<string>;
DEFINE FIELD parent_count ON type_node TYPE option<int>;
DEFINE FIELD child_count ON type_node TYPE option<int>;
DEFINE FIELD status ON type_node TYPE option<string>;
DEFINE FIELD source ON type_node TYPE option<string>;

DEFINE INDEX type_node_code_idx ON type_node FIELDS code UNIQUE;

DEFINE TABLE type_edge SCHEMAFULL
    PERMISSIONS
        FULL;

DEFINE FIELD in ON type_edge TYPE record(type_node);
DEFINE FIELD out ON type_edge TYPE record(type_node);
DEFINE FIELD depth_hint ON type_edge TYPE option<int>;
DEFINE FIELD source ON type_edge TYPE option<string>;
DEFINE FIELD created_at ON type_edge TYPE option<datetime>;

DEFINE INDEX type_edge_in_idx ON type_edge FIELDS in;
DEFINE INDEX type_edge_out_idx ON type_edge FIELDS out;

DEFINE TABLE type_path SCHEMAFULL
    PERMISSIONS
        FULL;

DEFINE FIELD ancestor_code ON type_path TYPE string;
DEFINE FIELD descendant_code ON type_path TYPE string;
DEFINE FIELD depth ON type_path TYPE int
    ASSERT $value >= 1;
DEFINE FIELD path ON type_path TYPE option<array<string>>;
DEFINE FIELD source ON type_path TYPE option<string>;

DEFINE INDEX type_path_lookup_idx ON type_path FIELDS ancestor_code, descendant_code UNIQUE;
DEFINE INDEX type_path_ancestor_idx ON type_path FIELDS ancestor_code;
DEFINE INDEX type_path_descendant_idx ON type_path FIELDS descendant_code;
