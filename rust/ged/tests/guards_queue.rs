//! Pools are explicit. A `*` pool, or a [`ged::jobs::queue::QueueName`] that
//! `ged worker` does not drain, would leave jobs sitting forever. The set of
//! queues is [`QueueName`], not a YAML file.

use std::str::FromStr;

use ged::jobs::queue::QueueName;
use ged::jobs::worker::WorkerSettings;

fn covered(settings: &WorkerSettings) -> Vec<QueueName> {
    settings
        .runners()
        .into_iter()
        .map(|runner| runner.queue)
        .collect()
}

#[test]
fn every_queue_is_drained_and_there_is_no_star_pool() {
    let settings = WorkerSettings::from_lookup(&|_| None);
    let drained = covered(&settings);
    for queue in QueueName::ALL {
        assert!(
            drained.contains(&queue),
            "queue {} has no worker pool",
            queue.as_str()
        );
    }
    assert!(
        QueueName::from_str("*").is_err(),
        "a '*' pool must not be representable"
    );
}

#[test]
fn scaled_uploads_pool_still_drains_default() {
    let settings = WorkerSettings::from_lookup(&|key| match key {
        "JOB_THREADS" => Some("5".into()),
        "JOB_CONCURRENCY" => Some("3".into()),
        _ => None,
    });
    let drained = covered(&settings);
    assert!(drained.contains(&QueueName::Uploads));
    assert!(drained.contains(&QueueName::Default));
    assert!(drained.iter().filter(|q| **q == QueueName::Uploads).count() >= 3);
}
