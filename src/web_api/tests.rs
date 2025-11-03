#[cfg(test)]
mod tests {
    use crate::web_api::spatial_query_api::{SpatialNode, determine_node_type};

    #[test]
    fn test_determine_node_type_space() {
        assert_eq!(determine_node_type("FRMW"), "SPACE");
        assert_eq!(determine_node_type("SBFR"), "SPACE");
    }

    #[test]
    fn test_determine_node_type_room() {
        assert_eq!(determine_node_type("PANE"), "ROOM");
    }

    #[test]
    fn test_determine_node_type_component() {
        assert_eq!(determine_node_type("PIPE"), "COMPONENT");
        assert_eq!(determine_node_type("ELBO"), "COMPONENT");
        assert_eq!(determine_node_type("EQUI"), "COMPONENT");
        assert_eq!(determine_node_type("NOZL"), "COMPONENT");
        assert_eq!(determine_node_type("FLNG"), "COMPONENT");
        assert_eq!(determine_node_type("TEES"), "COMPONENT");
        assert_eq!(determine_node_type("REDU"), "COMPONENT");
        assert_eq!(determine_node_type("VALV"), "COMPONENT");
        assert_eq!(determine_node_type("INST"), "COMPONENT");
    }

    #[test]
    fn test_determine_node_type_unknown() {
        assert_eq!(determine_node_type("UNKNOWN"), "COMPONENT");
    }

    #[test]
    fn test_spatial_node_creation() {
        let node = SpatialNode {
            refno: 12345,
            name: "Test Node".to_string(),
            noun: "PIPE".to_string(),
            node_type: "COMPONENT".to_string(),
            children_count: 5,
        };

        assert_eq!(node.refno, 12345);
        assert_eq!(node.name, "Test Node");
        assert_eq!(node.noun, "PIPE");
        assert_eq!(node.node_type, "COMPONENT");
        assert_eq!(node.children_count, 5);
    }
}
