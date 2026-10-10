//! Integration tests for perch-github, in one binary so that the crate and
//! its dependencies (reqwest, octocrab) link once.

mod support;

mod comment_deletion;
mod concurrency;
mod conditional_reads;
mod description_edit;
mod discussion;
mod issues;
mod notifications;
mod pull_merge;
mod pull_reviews;
mod reactions;
mod releases;
mod search;
mod timeline_pages;
mod workflow_runs;
