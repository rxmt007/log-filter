use logcore::problems::{
    public_line_number, GroupQuery, PageSpec, ProblemEvent, ProblemEventId, ProblemGroupSummary,
    ProblemKind,
};
use logcore::session::Session;
use std::fmt::Write as _;
use std::io::Write as _;

const MIXED_POSITIVE: &str = include_str!("fixtures/problems/mixed_positive.log");
const MIXED_POSITIVE_GOLDEN: &str = include_str!("fixtures/problems/mixed_positive.golden");
const HIGH_SIMILARITY_NEGATIVE: &str =
    include_str!("fixtures/problems/high_similarity_negative.log");
const HIGH_SIMILARITY_NEGATIVE_GOLDEN: &str =
    include_str!("fixtures/problems/high_similarity_negative.golden");

const SCAN_CHUNKS: [usize; 3] = [1, 4_096, usize::MAX];

#[test]
fn mixed_positive_fixture_matches_golden_for_every_scan_chunk() {
    for chunk in SCAN_CHUNKS {
        let actual = analyze_and_render(MIXED_POSITIVE, chunk);
        assert_eq!(
            actual, MIXED_POSITIVE_GOLDEN,
            "mixed fixture changed for scan chunk {chunk}"
        );
    }
}

#[test]
fn high_similarity_negative_fixture_stays_empty_for_every_scan_chunk() {
    for chunk in SCAN_CHUNKS {
        let actual = analyze_and_render(HIGH_SIMILARITY_NEGATIVE, chunk);
        assert_eq!(
            actual, HIGH_SIMILARITY_NEGATIVE_GOLDEN,
            "negative fixture changed for scan chunk {chunk}"
        );
    }
}

fn analyze_and_render(fixture: &str, chunk: usize) -> String {
    let mut source = tempfile::NamedTempFile::new().expect("create fixture file");
    source
        .write_all(fixture.as_bytes())
        .expect("write fixture file");
    source.flush().expect("flush fixture file");

    let mut session = Session::open(source.path()).expect("open fixture session");
    session.index_all();

    let mut previous = 0;
    loop {
        let step = session.scan_problems_step(chunk);
        if step.caught_up {
            break;
        }
        assert!(
            step.scanned_lines > previous,
            "problem scan must advance before it catches up"
        );
        previous = step.scanned_lines;
    }
    assert!(
        session.finish_problem_input().finished,
        "static fixture must finish after reaching the indexed frontier"
    );

    render_public_snapshot(&mut session)
}

fn render_public_snapshot(session: &mut Session) -> String {
    let stats = session.problem_stats();
    let mut output = String::new();
    writeln!(
        output,
        "stats observed={} stored={} groups={} dropped={} correlation_limited={} identity_limited={}",
        stats.observed_occurrence_count,
        stats.stored_occurrence_count,
        stats.stored_group_count,
        stats.dropped_occurrence_count,
        stats.correlation_limited,
        stats.identity_coverage_limited,
    )
    .unwrap();

    let snapshot = session
        .create_problem_group_snapshot(&GroupQuery::default())
        .expect("create group snapshot");
    let mut groups = session
        .problem_group_snapshot_page(snapshot, PageSpec::new(0, 200).unwrap())
        .expect("read group snapshot")
        .items;
    groups.sort_by(|left, right| {
        kind_order(left.key.kind())
            .cmp(&kind_order(right.key.kind()))
            .then_with(|| {
                left.process_summary
                    .as_str()
                    .cmp(right.process_summary.as_str())
            })
            .then_with(|| {
                left.signature_summary
                    .as_str()
                    .cmp(right.signature_summary.as_str())
            })
    });

    for group in groups {
        render_group(&mut output, session, group);
    }
    output
}

fn kind_order(kind: ProblemKind) -> u8 {
    match kind {
        ProblemKind::JavaCrash => 0,
        ProblemKind::JavaOom => 1,
        ProblemKind::Anr => 2,
        ProblemKind::NativeCrash => 3,
        ProblemKind::ProcessRestart => 4,
        ProblemKind::SignalExit => 5,
        ProblemKind::LmkKill => 6,
        ProblemKind::KernelOomKill => 7,
    }
}

fn render_group(output: &mut String, session: &mut Session, group: ProblemGroupSummary) {
    writeln!(
        output,
        "group id={} kind={:?} fingerprint={} quality={:?}/{:?} process={:?} signature={:?} count={}/{}/{} first={} last={} first_ts={} last_ts={}",
        group.id.raw(),
        group.key.kind(),
        group.key.fingerprint().to_hex(),
        group.key.signature_quality(),
        group.key.identity_quality(),
        group.process_summary.as_str(),
        group.signature_summary.as_str(),
        group.observed_occurrence_count,
        group.stored_occurrence_count,
        group.dropped_occurrence_count,
        public_line_number(group.first_observed_line),
        public_line_number(group.last_observed_line),
        group.first_observed_timestamp.raw(),
        group.last_observed_timestamp.raw(),
    )
    .unwrap();

    let snapshot = session
        .create_problem_occurrence_snapshot(group.id)
        .expect("create occurrence snapshot");
    let event_ids = session
        .problem_occurrence_snapshot_page(snapshot, PageSpec::new(0, 200).unwrap())
        .expect("read occurrence snapshot")
        .items;
    for event_id in event_ids {
        let event = session
            .problem_event(event_id)
            .expect("snapshot event must remain addressable");
        render_event(output, session, event_id, event);
    }
}

fn render_event(
    output: &mut String,
    session: &Session,
    event_id: ProblemEventId,
    event: ProblemEvent,
) {
    writeln!(
        output,
        "  event id={} kind={:?} range={}..={} anchor={} pid={} instance={} group={} observations={}/{} evidence=0x{:x} outcome=0x{:x} boundary=0x{:x}",
        event_id.0,
        event.kind(),
        public_line_number(event.start_line()),
        public_line_number(event.end_line()),
        public_line_number(event.anchor_line()),
        event.pid(),
        event.process_instance().0,
        event.group_id_raw(),
        event.observation_len(),
        event.observation_total(),
        event.evidence().bits(),
        event.outcome().bits(),
        event.boundary().bits(),
    )
    .unwrap();

    for observation in session
        .problem_event_observations(event_id)
        .expect("stored event must expose its materialized facts")
    {
        writeln!(
            output,
            "    fact line={} code={:?} rule={:?} role={:?} format={:?} provenance={:?}",
            public_line_number(observation.line()),
            observation.fact(),
            observation.rule(),
            observation.role(),
            observation.format(),
            observation.provenance(),
        )
        .unwrap();
    }
}
