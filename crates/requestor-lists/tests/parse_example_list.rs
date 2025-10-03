use requestor_lists::RequestorList;

#[test]
fn test_parse_boundless_recommended_list() {
    let json = include_str!("../../../requestor-lists/boundless-recommended-priority-list.json");
    let list = RequestorList::from_json(json).expect("Failed to parse list");

    assert_eq!(list.name, "Boundless Recommended Priority List");
    assert_eq!(list.schema_version.major, 1);
    assert_eq!(list.schema_version.minor, 0);
    assert_eq!(list.version.major, 1);
    assert_eq!(list.version.minor, 0);
    assert_eq!(list.requestors.len(), 4);
}
