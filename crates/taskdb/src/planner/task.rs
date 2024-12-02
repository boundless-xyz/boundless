// Copyright 2024 RISC Zero, Inc.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Finalize,
    Join,
    Segment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Task {
    pub task_number: usize,
    pub task_height: u32,
    pub command: Command,
    pub depends_on: Vec<usize>,
}

impl Task {
    pub fn new_segment(task_number: usize) -> Self {
        Task { task_number, task_height: 0, command: Command::Segment, depends_on: Vec::new() }
    }

    pub fn new_join(task_number: usize, task_height: u32, left: usize, right: usize) -> Self {
        Task { task_number, task_height, command: Command::Join, depends_on: vec![left, right] }
    }

    pub fn new_finalize(task_number: usize, task_height: u32, depends_on: usize) -> Self {
        Task { task_number, task_height, command: Command::Finalize, depends_on: vec![depends_on] }
    }
}
