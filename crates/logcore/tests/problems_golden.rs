use logcore::problems::{
    public_line_number, GroupQuery, LogBuffer, PageSpec, ProblemEvent, ProblemEventId,
    ProblemGroupSummary, ProblemKind, SourceSpan,
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
const JAVA_MATRIX: &str = include_str!("fixtures/problems/java/matrix.log");
const JAVA_MATRIX_GOLDEN: &str = include_str!("fixtures/problems/java/matrix.golden");
const JAVA_OOM_MATRIX: &str = include_str!("fixtures/problems/memory/java-oom/matrix.log");
const JAVA_OOM_MATRIX_GOLDEN: &str =
    include_str!("fixtures/problems/memory/java-oom/matrix.golden");
const ANR_MATRIX: &str = include_str!("fixtures/problems/anr/matrix.log");
const ANR_MATRIX_GOLDEN: &str = include_str!("fixtures/problems/anr/matrix.golden");
const NATIVE_MATRIX: &str = include_str!("fixtures/problems/native/matrix.log");
const NATIVE_MATRIX_GOLDEN: &str = include_str!("fixtures/problems/native/matrix.golden");
const LIFECYCLE_MATRIX: &str = include_str!("fixtures/problems/lifecycle/matrix.log");
const LIFECYCLE_MATRIX_GOLDEN: &str = include_str!("fixtures/problems/lifecycle/matrix.golden");
const MEMORY_MATRIX: &str = include_str!("fixtures/problems/memory/matrix.log");
const MEMORY_MATRIX_GOLDEN: &str = include_str!("fixtures/problems/memory/matrix.golden");
const KERNEL_OOM_MATRIX: &str = include_str!("fixtures/problems/memory/kernel-oom/matrix.log");
const KERNEL_OOM_MATRIX_GOLDEN: &str =
    include_str!("fixtures/problems/memory/kernel-oom/matrix.golden");
const CONTINUATION_INTERLEAVED: &str =
    include_str!("fixtures/problems/incremental/continuation_interleaved.log");
const CONTINUATION_INTERLEAVED_GOLDEN: &str =
    include_str!("fixtures/problems/incremental/continuation_interleaved.golden");

const SCAN_CHUNKS: [usize; 3] = [1, 4_096, usize::MAX];
const MIXED_POSITIVE_SPANS: &[(u32, u32, LogBuffer)] = &[
    (1, 16, LogBuffer::Main),
    (18, 21, LogBuffer::System),
    (23, 26, LogBuffer::Events),
    (28, 28, LogBuffer::Main),
    (30, 34, LogBuffer::Crash),
    (36, 36, LogBuffer::Events),
    (38, 38, LogBuffer::Kernel),
];

#[test]
fn mixed_positive_fixture_matches_golden_for_every_scan_chunk() {
    for chunk in SCAN_CHUNKS {
        let actual = analyze_and_render(MIXED_POSITIVE, chunk, MIXED_POSITIVE_SPANS);
        assert_eq!(
            actual, MIXED_POSITIVE_GOLDEN,
            "mixed fixture changed for scan chunk {chunk}"
        );
    }
}

#[test]
fn high_similarity_negative_fixture_stays_empty_for_every_scan_chunk() {
    for chunk in SCAN_CHUNKS {
        let actual = analyze_and_render(HIGH_SIMILARITY_NEGATIVE, chunk, &[]);
        assert_eq!(
            actual, HIGH_SIMILARITY_NEGATIVE_GOLDEN,
            "negative fixture changed for scan chunk {chunk}"
        );
    }
}

#[test]
fn continuation_and_interleaving_fixture_matches_golden_for_every_scan_chunk() {
    for chunk in SCAN_CHUNKS {
        let actual = analyze_and_render(CONTINUATION_INTERLEAVED, chunk, &[]);
        assert_eq!(
            actual, CONTINUATION_INTERLEAVED_GOLDEN,
            "continuation/interleaving fixture changed for scan chunk {chunk}"
        );
    }
}

#[test]
fn growing_segmented_append_matches_static_public_snapshot_field_for_field() {
    let expected = analyze_and_render(CONTINUATION_INTERLEAVED, usize::MAX, &[]);
    let mut source = tempfile::NamedTempFile::new().expect("create growing fixture file");
    let mut session = Session::open_growing(source.path()).expect("open growing fixture session");
    let bytes = CONTINUATION_INTERLEAVED.as_bytes();
    let append_sizes = [1, 7, 31, 2, 113, 5, 257, 17];
    let mut offset = 0;
    let mut append_index = 0;

    while offset < bytes.len() {
        let end = offset
            .saturating_add(append_sizes[append_index % append_sizes.len()])
            .min(bytes.len());
        source
            .write_all(&bytes[offset..end])
            .expect("append growing fixture segment");
        source.flush().expect("flush growing fixture segment");
        session
            .remap_and_index_step(usize::MAX)
            .expect("remap and index growing fixture segment");
        scan_to_caught_up(&mut session, 1);
        offset = end;
        append_index += 1;
    }

    assert!(
        !session.finish_problem_input().finished,
        "growing input must not finalize pending events before seal"
    );
    session
        .seal_growing_input()
        .expect("seal fully indexed growing fixture");
    scan_to_caught_up(&mut session, 1);
    assert!(session.finish_problem_input().finished);

    assert_eq!(
        render_public_snapshot(&mut session),
        expected,
        "growing append changed a public group, event, or observation field"
    );
}

#[test]
fn per_kind_positive_and_high_similarity_negative_matrices_are_chunk_invariant() {
    let matrices = [
        (
            "java",
            JAVA_MATRIX,
            ProblemKind::JavaCrash,
            vec![(0, 19, LogBuffer::Events), (20, 40, LogBuffer::Main)],
            JAVA_MATRIX_GOLDEN,
        ),
        (
            "anr",
            ANR_MATRIX,
            ProblemKind::Anr,
            vec![(0, 19, LogBuffer::Events), (20, 40, LogBuffer::Main)],
            ANR_MATRIX_GOLDEN,
        ),
        (
            "java-oom",
            JAVA_OOM_MATRIX,
            ProblemKind::JavaOom,
            vec![
                (0, 21, LogBuffer::Main),
                (22, 30, LogBuffer::Events),
                (31, 58, LogBuffer::Main),
                (59, 63, LogBuffer::Events),
            ],
            JAVA_OOM_MATRIX_GOLDEN,
        ),
        (
            "native",
            NATIVE_MATRIX,
            ProblemKind::NativeCrash,
            vec![(0, 19, LogBuffer::Events), (20, 40, LogBuffer::Main)],
            NATIVE_MATRIX_GOLDEN,
        ),
        (
            "lifecycle",
            LIFECYCLE_MATRIX,
            ProblemKind::ProcessRestart,
            vec![(0, 39, LogBuffer::Events), (40, 60, LogBuffer::Main)],
            LIFECYCLE_MATRIX_GOLDEN,
        ),
        (
            "memory",
            MEMORY_MATRIX,
            ProblemKind::LmkKill,
            vec![
                (0, 15, LogBuffer::Main),
                (16, 19, LogBuffer::Kernel),
                (20, 40, LogBuffer::Main),
            ],
            MEMORY_MATRIX_GOLDEN,
        ),
        (
            "kernel-oom",
            KERNEL_OOM_MATRIX,
            ProblemKind::KernelOomKill,
            vec![(0, 32, LogBuffer::Kernel), (35, 42, LogBuffer::Kernel)],
            KERNEL_OOM_MATRIX_GOLDEN,
        ),
    ];

    for (name, fixture, expected_kind, source_spans, golden) in matrices {
        for chunk in SCAN_CHUNKS {
            let actual = analyze_and_render(fixture, chunk, &source_spans);
            assert_eq!(
                actual, golden,
                "{name} canonical public snapshot changed for scan chunk {chunk}"
            );
            assert_matrix_events(name, &actual, expected_kind, 10);
        }
    }
}

fn assert_matrix_events(name: &str, rendered: &str, expected_kind: ProblemKind, count: usize) {
    assert!(
        rendered.starts_with(&format!("stats observed={count} stored={count} groups=")),
        "{name} matrix did not commit exactly {count} occurrences:\n{rendered}"
    );
    let event_lines: Vec<_> = rendered
        .lines()
        .filter(|line| line.starts_with("  event id="))
        .collect();
    assert_eq!(
        event_lines.len(),
        count,
        "{name} matrix public snapshot:\n{rendered}"
    );
    assert!(
        event_lines
            .iter()
            .all(|line| line.contains(&format!("kind={expected_kind:?}"))),
        "{name} matrix contains an unexpected Problem kind:\n{rendered}"
    );
}

fn analyze_and_render(
    fixture: &str,
    chunk: usize,
    source_spans: &[(u32, u32, LogBuffer)],
) -> String {
    let mut source = tempfile::NamedTempFile::new().expect("create fixture file");
    source
        .write_all(fixture.as_bytes())
        .expect("write fixture file");
    source.flush().expect("flush fixture file");

    let mut session = Session::open(source.path()).expect("open fixture session");
    for &(start_line, end_line, buffer) in source_spans {
        session
            .add_problem_source_span(
                SourceSpan::new(start_line, end_line, buffer).expect("valid fixture source span"),
            )
            .expect("non-overlapping fixture source span");
    }
    session.index_all();

    scan_to_caught_up(&mut session, chunk);
    assert!(
        session.finish_problem_input().finished,
        "static fixture must finish after reaching the indexed frontier"
    );

    render_public_snapshot(&mut session)
}

fn scan_to_caught_up(session: &mut Session, chunk: usize) {
    let mut previous = session.problem_scanned_lines();
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
        "group id={} kind={:?} fingerprint_version={} fingerprint={} quality={:?}/{:?} process={:?} signature={:?} count={}/{}/{} first={} last={} first_ts={} last_ts={}",
        group.id.raw(),
        group.key.kind(),
        group.key.fingerprint_version(),
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
