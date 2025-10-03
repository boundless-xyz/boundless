use requestor_lists::RequestorList;

#[test]
fn test_parse_boundless_recommended_list() {
    let json = include_str!("../../../requestor-lists/boundless-priority-list.standard.json");
    let list = RequestorList::from_json(json).expect("Failed to parse list");

    assert_eq!(list.name, "Boundless Recommended Priority List");
    assert_eq!(
        list.description,
        "List of recommended priority requestors for provers. The requestors here should be suitable for most provers."
    );
    assert_eq!(list.schema_version.major, 1);
    assert_eq!(list.schema_version.minor, 0);
    assert_eq!(list.version.major, 1);
    assert_eq!(list.version.minor, 0);
    assert_eq!(list.requestors.len(), 1);
}
