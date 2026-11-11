//! Shrink a diverging request, then the short prefix that leads to it.
//!
//! The predicate is supplied by the binary: it replays against both stacks.
//! Candidates are strictly smaller, so the loop ends.

use super::model::{Auth, Body, FilePart, Request};

pub async fn minimize<F, Fut>(mut req: Request, mut diverges: F) -> Request
where
    F: FnMut(Request) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if !diverges(req.clone()).await {
        return req;
    }
    loop {
        let mut next = None;
        for trial in candidates(&req) {
            if weight(&trial) >= weight(&req) {
                continue;
            }
            if diverges(trial.clone()).await {
                next = Some(trial);
                break;
            }
        }
        match next {
            Some(trial) => req = trial,
            None => break,
        }
    }
    req
}

/// Drop earlier requests that are not required for the last one to diverge.
/// The last request is kept. `diverges` is expected to reset server state.
pub async fn shrink_sequence<F, Fut>(seq: Vec<Request>, mut diverges: F) -> (Vec<Request>, bool)
where
    F: FnMut(&[Request]) -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    if seq.is_empty() {
        return (seq, false);
    }
    if !diverges(&seq).await {
        return (seq, false);
    }
    let mut kept = seq;
    let mut i = 0;
    while i + 1 < kept.len() {
        let mut trial = Vec::with_capacity(kept.len() - 1);
        trial.extend(kept[..i].iter().cloned());
        trial.extend(kept[i + 1..].iter().cloned());
        if diverges(&trial).await {
            kept = trial;
        } else {
            i += 1;
        }
    }
    (kept, true)
}

/// Candidates that are strictly smaller than `req`. The minimizer tries them
/// in order and keeps the first one that still diverges.
pub fn smaller_candidates(req: &Request) -> Vec<Request> {
    let current = weight(req);
    candidates(req)
        .into_iter()
        .filter(|trial| weight(trial) < current)
        .collect()
}

fn weight(req: &Request) -> usize {
    let auth = match req.auth {
        Auth::None => 0,
        Auth::Valid | Auth::Unknown | Auth::NoGrant | Auth::Other => 1,
        Auth::Garbage | Auth::EmptyBearer | Auth::Basic => 2,
        Auth::Expired | Auth::BadAud => 3,
    };
    let body = match &req.body {
        Body::None => 0,
        Body::Raw { bytes, .. } => 10 + bytes.len(),
        Body::Form(fields) => 10 + fields.len(),
        Body::Multipart { fields, file, .. } => {
            20 + fields.len()
                + file
                    .as_ref()
                    .map(|f| f.bytes.len() + f.filename.len())
                    .unwrap_or(0)
        }
    };
    req.query.len()
        + req.extra_headers.len()
        + req
            .raw_query
            .as_ref()
            .map(|s| s.len().saturating_add(1))
            .unwrap_or(0)
        + req
            .raw_path
            .as_ref()
            .map(|s| s.len().saturating_add(1))
            .unwrap_or(0)
        + auth
        + body
}

fn candidates(req: &Request) -> Vec<Request> {
    let mut out = Vec::new();
    if req.raw_query.is_some() {
        let mut trial = req.clone();
        trial.raw_query = None;
        out.push(trial);
    }
    if req.raw_path.is_some() {
        let mut trial = req.clone();
        trial.raw_path = None;
        out.push(trial);
    }
    for index in 0..req.query.len() {
        let mut trial = req.clone();
        trial.query.remove(index);
        out.push(trial);
    }
    for index in 0..req.extra_headers.len() {
        let mut trial = req.clone();
        trial.extra_headers.remove(index);
        out.push(trial);
    }
    if req.auth != Auth::None {
        let mut trial = req.clone();
        trial.auth = Auth::None;
        out.push(trial);
    }
    if req.auth != Auth::Valid && req.auth != Auth::None {
        let mut trial = req.clone();
        trial.auth = Auth::Valid;
        out.push(trial);
    }
    match &req.body {
        Body::None => {}
        Body::Form(fields) => {
            for index in 0..fields.len() {
                let mut trial = req.clone();
                if let Body::Form(items) = &mut trial.body {
                    items.remove(index);
                }
                out.push(trial);
            }
            let mut cleared = req.clone();
            cleared.body = Body::None;
            out.push(cleared);
        }
        Body::Raw {
            bytes,
            content_type,
        } => {
            if bytes.len() > 1 {
                let mut trial = req.clone();
                trial.body = Body::Raw {
                    content_type: content_type.clone(),
                    bytes: bytes[..1].to_vec(),
                };
                out.push(trial);
            }
            if !bytes.is_empty() {
                let mut trial = req.clone();
                trial.body = Body::Raw {
                    content_type: content_type.clone(),
                    bytes: Vec::new(),
                };
                out.push(trial);
            }
            let mut cleared = req.clone();
            cleared.body = Body::None;
            out.push(cleared);
        }
        Body::Multipart {
            fields,
            file,
            boundary,
        } => {
            for index in 0..fields.len() {
                let mut trial = req.clone();
                if let Body::Multipart { fields, .. } = &mut trial.body {
                    fields.remove(index);
                }
                out.push(trial);
            }
            if let Some(file) = file {
                if file.filename != "a.txt" || file.bytes != b"x" {
                    let mut trial = req.clone();
                    if let Body::Multipart { file, .. } = &mut trial.body {
                        *file = Some(FilePart {
                            filename: "a.txt".into(),
                            content_type: "text/plain".into(),
                            bytes: b"x".to_vec(),
                        });
                    }
                    out.push(trial);
                }
                let mut no_file = req.clone();
                if let Body::Multipart { file, .. } = &mut no_file.body {
                    *file = None;
                }
                out.push(no_file);
            }
            if !fields.is_empty() || file.is_some() {
                let mut cleared = req.clone();
                cleared.body = Body::Multipart {
                    boundary: boundary.clone(),
                    fields: Vec::new(),
                    file: None,
                };
                out.push(cleared);
            }
            let mut none = req.clone();
            none.body = Body::None;
            out.push(none);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_fuzz::model::{IdRef, Route};

    fn sample() -> Request {
        let mut req = Request::get("list-documents", Route::Documents);
        req.query = vec![
            ("keep".into(), "1".into()),
            ("noise".into(), "x".into()),
            ("also".into(), "y".into()),
        ];
        req.extra_headers = vec![("X-Noise".into(), "1".into())];
        req.auth = Auth::Expired;
        req
    }

    #[tokio::test]
    async fn query_params_and_headers_that_do_not_carry_the_diff_are_dropped() {
        let out = minimize(sample(), |req| async move {
            req.query.iter().any(|(k, _)| k == "keep") && req.auth == Auth::Expired
        })
        .await;
        assert_eq!(out.query, vec![("keep".into(), "1".into())]);
        assert!(out.extra_headers.is_empty());
        assert_eq!(out.auth, Auth::Expired);
    }

    #[tokio::test]
    async fn a_request_that_does_not_diverge_is_left_unchanged() {
        let req = sample();
        let out = minimize(req.clone(), |_| async { false }).await;
        assert_eq!(out, req);
    }

    #[tokio::test]
    async fn the_sequence_shrinks_to_the_requests_the_diff_needs() {
        let mut prefix = Request::get("create-department", Route::Departments);
        prefix.method = "POST";
        let mut noise = Request::get("ping", Route::Ping);
        noise.auth = Auth::None;
        let mut last = Request::get(
            "show-document",
            Route::Document {
                action: crate::diff_fuzz::model::DocAction::Show,
                id: IdRef::Doc,
            },
        );
        last.raw_query = Some("a=%zz".into());
        let seq = vec![prefix, noise, last];
        let (kept, reproduced) = shrink_sequence(seq, |reqs| {
            let hit = reqs.len() >= 2
                && reqs
                    .first()
                    .is_some_and(|r| r.category == "create-department")
                && reqs.last().is_some_and(|r| r.raw_query.is_some());
            async move { hit }
        })
        .await;
        assert!(reproduced);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].category, "create-department");
        assert_eq!(kept[1].category, "show-document");
    }
}
