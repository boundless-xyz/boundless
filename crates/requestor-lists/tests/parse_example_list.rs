// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use requestor_lists::RequestorList;

#[test]
fn test_parse_boundless_recommended_list() {
    let json = include_str!("../../../requestor-lists/boundless-priority-list.standard.json");
    let list = RequestorList::from_json(json).expect("Failed to parse list");

    assert_eq!(list.name, "Boundless Recommended Priority List");
    assert_eq!(
        list.description,
        "List of recommended priority requestors for provers. The request sizes here should be suitable for most provers."
    );
    assert_eq!(list.schema_version.major, 1);
    assert_eq!(list.schema_version.minor, 0);
    assert_eq!(list.version.major, 1);
    assert_eq!(list.version.minor, 0);
    assert_eq!(list.requestors.len(), 1);
}
