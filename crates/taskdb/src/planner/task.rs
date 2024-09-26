// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

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
