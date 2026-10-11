use std::collections::BTreeMap;

use super::*;
use crate::accounts::{NewAccount, Provider};
use crate::campaigns::{CampaignInput, StepInput};
use crate::clock::FakeClock;
use crate::contacts::Contact;
use crate::providers::fake::FakeProvider;

struct FakeFactory(Arc<FakeProvider>);

impl ProviderFactory for FakeFactory {
    fn for_account(&self, _a: &Account, _dry: bool) -> Result<Arc<dyn MailProvider>, ProviderError> {
        Ok(self.0.clone())
    }
}

struct Harness {
    engine: Engine,
    clock: Arc<FakeClock>,
    fake: Arc<FakeProvider>,
    account_id: i64,
    campaign_id: i64,
}

// Mon 12 Oct 2026, 10:00 UTC.
const START: &str = "2026-10-12T10:00:00Z";

fn contact(email: &str, first: &str) -> Contact {
    Contact {
        id: 0,
        email: email.into(),
        first_name: first.into(),
        last_name: String::new(),
        fields: BTreeMap::new(),
        tags: vec![],
        lists: vec![],
        do_not_contact: false,
        created_at: String::new(),
    }
}

fn step(delay: i64, subject: &str, body: &str) -> StepInput {
    StepInput { id: None, delay_days: delay, subject: subject.into(), body: body.into(), same_thread: true }
}

/// Account + list of `emails` + a campaign with `steps`, all days 00:00–24:00.
fn setup(emails: &[&str], steps: Vec<StepInput>) -> Harness {
    let db = Db::open_in_memory().unwrap();
    let clock = Arc::new(FakeClock::at(START));
    let fake = Arc::new(FakeProvider::default());
    let (account_id, campaign_id) = {
        let mut conn = db.lock();
        let now = clock.now() - Duration::days(30);
        let a = accounts::upsert(
            &conn,
            &NewAccount { provider: Provider::Gmail, email: "me@gmail.com", display_name: "Me", account_type: "gmail_free" },
            now,
        )
        .unwrap();
        let list = contacts::create_list(&conn, "L", now).unwrap();
        for e in emails {
            let mut c = contact(e, "Ana");
            c.lists = vec![list];
            contacts::save(&conn, &c, now).unwrap();
        }
        let c = campaigns::save(
            &mut conn,
            &CampaignInput {
                id: None,
                name: "Test".into(),
                account_id: Some(a.id),
                list_id: Some(list),
                window: SendWindow { days: [true; 7], start_min: 0, end_min: 24 * 60 },
                gap_min_secs: 60,
                gap_max_secs: 60,
                footer: "Reply stop to opt out.".into(),
                steps,
            },
            now,
        )
        .unwrap();
        (a.id, c.id)
    };
    let engine = Engine::new(
        db,
        clock.clone(),
        Arc::new(FakeFactory(fake.clone())),
        Arc::new(NoopNotifier),
        Zone::Fixed(FixedOffset::east_opt(0).unwrap()),
    );
    Harness { engine, clock, fake, account_id, campaign_id }
}

impl Harness {
    fn launch(&self) {
        let now = self.clock.now();
        campaigns::launch(&mut self.engine.db.lock(), self.campaign_id, now).unwrap();
    }
    async fn tick(&self) -> TickReport {
        self.engine.tick().await.unwrap()
    }
    fn advance(&self, secs: i64) {
        self.clock.advance(Duration::seconds(secs));
    }
    fn enrolment_status(&self, email: &str) -> String {
        self.engine
            .db
            .lock()
            .query_row(
                "SELECT e.status FROM enrolments e JOIN contacts c ON c.id = e.contact_id WHERE c.email = ?1",
                [email],
                |r| r.get(0),
            )
            .unwrap()
    }
    fn outbound_count(&self) -> i64 {
        self.engine.db.lock().query_row("SELECT COUNT(*) FROM outbound WHERE status = 'sent'", [], |r| r.get(0)).unwrap()
    }
    fn account(&self) -> Account {
        accounts::get(&self.engine.db.lock(), self.account_id).unwrap()
    }
    fn reply_raw(&self, from: &str, in_reply_to: &str, body: &str) -> IncomingEmail {
        let raw = format!(
            "From: Someone <{from}>\r\nTo: me@gmail.com\r\nSubject: Re: Hello\r\nMessage-ID: <{}@x.com>\r\nIn-Reply-To: {in_reply_to}\r\nReferences: {in_reply_to}\r\nContent-Type: text/plain\r\n\r\n{body}\r\n",
            uuid::Uuid::new_v4().simple()
        );
        IncomingEmail {
            provider_id: uuid::Uuid::new_v4().to_string(),
            raw: raw.into_bytes(),
            conversation_id: None,
            thread_id: None,
            web_link: None,
            received_at: None,
        }
    }
}

#[tokio::test]
async fn sends_step_one_then_follow_up_in_same_thread() {
    let h = setup(&["a@x.com", "b@x.com"], vec![step(0, "Hello {{first_name}}", "Hi {{first_name}}"), step(2, "", "Bumping this")]);
    h.launch();
    assert_eq!(h.tick().await.sent, 1, "one send per account per tick");
    assert_eq!(h.tick().await.sent, 0, "gap not elapsed");
    h.advance(61);
    assert_eq!(h.tick().await.sent, 1);
    h.advance(61);
    assert_eq!(h.tick().await.sent, 0, "follow-ups not due yet");

    let sent = h.fake.sent.lock().unwrap().clone();
    assert_eq!(sent[0].subject, "Hello Ana");
    assert_eq!(sent[0].body, "Hi Ana\n\nReply stop to opt out.\n");
    assert!(sent[0].thread.is_none());
    assert!(sent[0].list_unsubscribe.as_deref().unwrap().starts_with("mailto:me@gmail.com"));

    h.advance(2 * 86400);
    assert_eq!(h.tick().await.sent, 1);
    let follow = h.fake.sent.lock().unwrap()[2].clone();
    assert_eq!(follow.to_email, "a@x.com");
    assert_eq!(follow.subject, "Re: Hello Ana");
    let thread = follow.thread.expect("same thread");
    assert_eq!(thread.references, vec![sent[0].message_id.clone()]);
    assert_eq!(h.enrolment_status("a@x.com"), "finished");
    assert_eq!(h.enrolment_status("b@x.com"), "waiting");
}

#[tokio::test]
async fn never_sends_the_same_step_twice_after_crash() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    // Simulate a crash right after the "sending" marker was written: prepare
    // but never call the provider.
    let account = h.account();
    let job = h.engine.next_job(&account, h.clock.now()).unwrap().unwrap();
    let Prepared::Send { email, .. } = h.engine.prepare(&account, &job, h.clock.now()).unwrap() else { panic!() };
    // The provider did actually send it before the crash.
    h.fake.already_sent.lock().unwrap().push(email.message_id.clone());

    // Restart: ticks before the reconcile delay must not resend.
    assert_eq!(h.tick().await.sent, 0);
    h.advance(RECONCILE_AFTER_SECS + 1);
    assert_eq!(h.tick().await.sent, 0);
    assert!(h.fake.sent.lock().unwrap().is_empty(), "found in Sent, never resent");
    assert_eq!(h.outbound_count(), 1);
    assert_eq!(h.enrolment_status("a@x.com"), "finished");
}

#[tokio::test]
async fn crash_before_provider_accepted_retries_with_same_message_id() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    let account = h.account();
    let job = h.engine.next_job(&account, h.clock.now()).unwrap().unwrap();
    let Prepared::Send { email, .. } = h.engine.prepare(&account, &job, h.clock.now()).unwrap() else { panic!() };
    h.advance(RECONCILE_AFTER_SECS + 1);
    h.tick().await; // reconcile: not in Sent -> retry after backoff
    h.advance(11 * 60);
    assert_eq!(h.tick().await.sent, 1);
    let sent = h.fake.sent.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].message_id, email.message_id);
}

#[tokio::test]
async fn uncertain_failure_is_checked_before_retry() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    *h.fake.ghost_next.lock().unwrap() = true; // goes out, but we hear "timeout"
    assert_eq!(h.tick().await.failed, 1);
    for _ in 0..5 {
        h.advance(15 * 60);
        h.tick().await;
    }
    assert_eq!(h.fake.sent.lock().unwrap().len(), 1, "no duplicate");
    assert_eq!(h.outbound_count(), 1);
    assert_eq!(h.enrolment_status("a@x.com"), "finished");
}

#[tokio::test]
async fn outbound_unique_per_step_is_enforced_by_the_database() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.tick().await;
    let conn = h.engine.db.lock();
    let dup = conn.execute(
        "INSERT INTO outbound (enrolment_id, step_number, account_id, message_id, to_email, subject, status, attempted_at)
         SELECT enrolment_id, step_number, account_id, '<other@x>', to_email, subject, 'sending', attempted_at FROM outbound",
        [],
    );
    assert!(dup.is_err());
}

#[tokio::test]
async fn never_emails_do_not_contact() {
    let h = setup(&["a@x.com", "b@x.com", "c@x.com"], vec![step(0, "Hello", "Hi")]);
    {
        let conn = h.engine.db.lock();
        contacts::add_dnc(&conn, "a@x.com", "manual", h.clock.now()).unwrap();
    }
    h.launch();
    assert_eq!(h.enrolment_status("a@x.com"), "skipped", "skipped at launch");
    // Added to the list after launch: must still never be emailed.
    {
        let conn = h.engine.db.lock();
        contacts::add_dnc(&conn, "B@X.com", "manual", h.clock.now()).unwrap();
    }
    for _ in 0..5 {
        h.tick().await;
        h.advance(61);
    }
    assert_eq!(h.fake.sent_to(), vec!["c@x.com"]);
    assert_eq!(h.enrolment_status("b@x.com"), "skipped");
}

#[tokio::test]
async fn daily_limit_and_gradual_catch_up() {
    let emails: Vec<String> = (0..5).map(|i| format!("p{i}@x.com")).collect();
    let refs: Vec<&str> = emails.iter().map(|s| s.as_str()).collect();
    let h = setup(&refs, vec![step(0, "Hello", "Hi")]);
    h.engine.db.lock().execute("UPDATE accounts SET daily_limit = 2", []).unwrap();
    h.launch();
    // App closed for three days: on reopening, no burst.
    h.advance(3 * 86400);
    assert_eq!(h.tick().await.sent, 1);
    assert_eq!(h.tick().await.sent, 0);
    h.advance(61);
    assert_eq!(h.tick().await.sent, 1);
    h.advance(61);
    assert_eq!(h.tick().await.sent, 0, "daily limit reached");
    h.advance(86400);
    assert_eq!(h.tick().await.sent, 1, "window cleared");
}

#[tokio::test]
async fn provider_limit_pauses_account_without_retry_storm() {
    let h = setup(&["a@x.com", "b@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.fake.fail_next.lock().unwrap().push_back(ProviderError::LimitReached("limit".into()));
    assert_eq!(h.tick().await.failed, 1);
    for _ in 0..10 {
        h.advance(120);
        assert_eq!(h.tick().await, TickReport::default());
    }
    assert!(h.fake.sent.lock().unwrap().is_empty());
    assert!(h.account().limit_paused_until.is_some());
    h.advance(25 * 3600);
    assert_eq!(h.tick().await.sent, 1, "resumes after the window clears");
}

#[tokio::test]
async fn auth_failure_marks_account_and_resends_nothing_twice() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.fake.fail_next.lock().unwrap().push_back(ProviderError::Auth("Sign in again".into()));
    h.tick().await;
    assert_eq!(h.account().status, "needs_attention");
    h.advance(3600);
    assert_eq!(h.tick().await.sent, 0, "paused while it needs attention");
    accounts::set_status(&h.engine.db.lock(), h.account_id, "connected", None).unwrap();
    assert_eq!(h.tick().await.sent, 1);
    h.advance(3600);
    assert_eq!(h.tick().await.sent, 0);
    assert_eq!(h.fake.sent.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rejected_recipient_fails_only_that_contact() {
    let h = setup(&["a@x.com", "b@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.fake.fail_next.lock().unwrap().push_back(ProviderError::Rejected("no such user".into()));
    h.tick().await;
    h.advance(61);
    h.tick().await;
    assert_eq!(h.enrolment_status("a@x.com"), "failed");
    assert_eq!(h.enrolment_status("b@x.com"), "finished");
}

#[tokio::test]
async fn missing_merge_field_skips_contact() {
    let h = setup(&["a@x.com", "b@x.com"], vec![step(0, "Hello", "Hi {{company}}")]);
    h.engine.db.lock().execute("UPDATE contacts SET fields = '{\"company\":\"Acme\"}' WHERE email = 'b@x.com'", []).unwrap();
    h.launch();
    assert_eq!(h.enrolment_status("a@x.com"), "skipped");
    for _ in 0..3 {
        h.tick().await;
        h.advance(61);
    }
    assert_eq!(h.fake.sent_to(), vec!["b@x.com"]);
    assert!(h.fake.sent.lock().unwrap()[0].body.starts_with("Hi Acme"));
}

#[tokio::test]
async fn respects_send_window() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.engine
        .db
        .lock()
        .execute("UPDATE campaigns SET window_days = '1,2,3,4,5', window_start = 540, window_end = 1020", [])
        .unwrap();
    h.clock.set(parse_ts("2026-10-17T10:00:00Z").unwrap()); // Saturday
    h.launch();
    assert_eq!(h.tick().await.sent, 0);
    h.clock.set(parse_ts("2026-10-19T08:59:00Z").unwrap()); // Monday, before 9
    assert_eq!(h.tick().await.sent, 0);
    h.clock.set(parse_ts("2026-10-19T09:00:00Z").unwrap());
    assert_eq!(h.tick().await.sent, 1);
}

#[tokio::test]
async fn reply_stops_the_sequence() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi"), step(2, "", "Follow up")]);
    h.launch();
    h.tick().await;
    let mid = h.fake.sent.lock().unwrap()[0].message_id.clone();
    let account = h.account();
    assert!(h.engine.ingest(&account, &h.reply_raw("a@x.com", &mid, "Sounds good, call me")).unwrap());
    assert_eq!(h.enrolment_status("a@x.com"), "replied");
    h.advance(3 * 86400);
    h.tick().await;
    assert_eq!(h.fake.sent.lock().unwrap().len(), 1, "no follow-up after a reply");
    // The same message seen again is ignored.
    let stats = campaigns::stats(&h.engine.db.lock(), h.campaign_id).unwrap();
    assert_eq!((stats.replied, stats.contacted), (1, 1));
    assert!((stats.reply_rate - 1.0).abs() < 1e-9);
}

#[tokio::test]
async fn someone_else_replying_in_thread_also_stops() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi"), step(2, "", "Follow up")]);
    h.launch();
    h.tick().await;
    let mid = h.fake.sent.lock().unwrap()[0].message_id.clone();
    h.engine.ingest(&h.account(), &h.reply_raw("colleague@x.com", &mid, "Ana is away, I'll help")).unwrap();
    assert_eq!(h.enrolment_status("a@x.com"), "replied");
}

#[tokio::test]
async fn auto_reply_does_not_stop_but_bounce_and_unsubscribe_do() {
    let h = setup(&["a@x.com", "b@x.com", "c@x.com"], vec![step(0, "Hello", "Hi"), step(1, "", "Follow up")]);
    h.launch();
    for _ in 0..3 {
        h.tick().await;
        h.advance(61);
    }
    let sent = h.fake.sent.lock().unwrap().clone();
    let account = h.account();
    let id_for = |to: &str| sent.iter().find(|e| e.to_email == to).unwrap().message_id.clone();

    // a: out of office
    let mut ooo = h.reply_raw("a@x.com", &id_for("a@x.com"), "I'm away until Monday");
    ooo.raw = String::from_utf8(ooo.raw).unwrap().replacen("Subject: Re: Hello", "Subject: Automatic reply: Hello\r\nAuto-Submitted: auto-replied", 1).into_bytes();
    h.engine.ingest(&account, &ooo).unwrap();
    assert_eq!(h.enrolment_status("a@x.com"), "waiting");

    // b: bounce
    let bounce = format!(
        "From: Mail Delivery Subsystem <mailer-daemon@googlemail.com>\r\nTo: me@gmail.com\r\nSubject: Delivery Status Notification (Failure)\r\nMessage-ID: <b1@google.com>\r\nContent-Type: multipart/report; report-type=delivery-status; boundary=\"XX\"\r\n\r\n--XX\r\nContent-Type: text/plain\r\n\r\nAddress not found\r\n--XX\r\nContent-Type: message/delivery-status\r\n\r\nFinal-Recipient: rfc822; b@x.com\r\nAction: failed\r\n--XX\r\nContent-Type: text/rfc822-headers\r\n\r\nMessage-ID: {}\r\nTo: b@x.com\r\n--XX--\r\n",
        id_for("b@x.com")
    );
    h.engine
        .ingest(&account, &IncomingEmail { provider_id: "b".into(), raw: bounce.into_bytes(), conversation_id: None, thread_id: None, web_link: None, received_at: None })
        .unwrap();
    assert_eq!(h.enrolment_status("b@x.com"), "bounced");

    // c: "stop"
    h.engine.ingest(&account, &h.reply_raw("c@x.com", &id_for("c@x.com"), "STOP")).unwrap();
    assert_eq!(h.enrolment_status("c@x.com"), "unsubscribed");

    {
        let conn = h.engine.db.lock();
        assert!(contacts::is_dnc(&conn, "b@x.com").unwrap());
        assert!(contacts::is_dnc(&conn, "c@x.com").unwrap());
        assert!(!contacts::is_dnc(&conn, "a@x.com").unwrap());
    }

    h.advance(2 * 86400);
    for _ in 0..3 {
        h.tick().await;
        h.advance(61);
    }
    let follow_ups: Vec<String> = h.fake.sent_to().into_iter().skip(3).collect();
    assert_eq!(follow_ups, vec!["a@x.com"]);
    let stats = campaigns::stats(&h.engine.db.lock(), h.campaign_id).unwrap();
    assert_eq!((stats.bounced, stats.unsubscribed, stats.auto_replies), (1, 1, 1));
}

#[tokio::test]
async fn unrelated_mail_is_ignored() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.tick().await;
    let account = h.account();
    assert!(!h.engine.ingest(&account, &h.reply_raw("x@y.com", "<random@elsewhere>", "Newsletter")).unwrap());
}

#[tokio::test]
async fn paused_campaign_sends_nothing_and_edits_apply_to_unsent() {
    let h = setup(&["a@x.com", "b@x.com"], vec![step(0, "Hello", "Hi")]);
    h.launch();
    h.tick().await;
    campaigns::set_status(&h.engine.db.lock(), h.campaign_id, "paused", h.clock.now()).unwrap();
    h.advance(61);
    assert_eq!(h.tick().await.sent, 0);
    {
        let mut conn = h.engine.db.lock();
        let c = campaigns::get(&conn, h.campaign_id).unwrap();
        campaigns::save(
            &mut conn,
            &CampaignInput {
                id: Some(c.id),
                name: c.name.clone(),
                account_id: c.account_id,
                list_id: c.list_id,
                window: c.window.clone(),
                gap_min_secs: 60,
                gap_max_secs: 60,
                footer: String::new(),
                steps: vec![StepInput { id: Some(c.steps[0].id), delay_days: 0, subject: "New subject".into(), body: "New body".into(), same_thread: false }],
            },
            h.clock.now(),
        )
        .unwrap();
        campaigns::set_status(&conn, h.campaign_id, "running", h.clock.now()).unwrap();
    }
    assert_eq!(h.tick().await.sent, 1);
    assert_eq!(h.fake.sent.lock().unwrap()[1].subject, "New subject");
}

#[tokio::test]
async fn send_test_goes_to_self_and_counts_toward_limit() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello {{first_name}}", "Hi {{nickname}}")]);
    let to = h.engine.send_test(h.campaign_id, 1, None).await.unwrap();
    assert_eq!(to, "me@gmail.com");
    let sent = h.fake.sent.lock().unwrap()[0].clone();
    assert_eq!(sent.to_email, "me@gmail.com");
    assert_eq!(sent.subject, "[Test] Hello Ana");
    assert!(sent.body.contains("[nickname]"));
    assert_eq!(accounts::sends_last_24h(&h.engine.db.lock(), h.account_id, h.clock.now()).unwrap(), 1);
}

#[tokio::test]
async fn dry_run_writes_eml_and_never_touches_the_network() {
    let h = setup(&["a@x.com"], vec![step(0, "Hello {{first_name}}", "Hi {{first_name}}")]);
    let dir = tempfile::tempdir().unwrap();
    {
        let conn = h.engine.db.lock();
        let mut s = crate::app::load_settings(&conn);
        s.dry_run = true;
        crate::app::save_settings(&conn, &s).unwrap();
    }
    // The real factory, with an empty secret store: if it tried to reach Gmail
    // it would fail for want of a password.
    let engine = Engine::new(
        h.engine.db.clone(),
        h.clock.clone(),
        Arc::new(crate::app::AppProviders {
            db: h.engine.db.clone(),
            secrets: Arc::new(crate::secrets::MemoryStore::default()),
            tokens: Arc::default(),
            http: reqwest::Client::new(),
            outbox_dir: dir.path().to_path_buf(),
        }),
        Arc::new(NoopNotifier),
        Zone::Fixed(FixedOffset::east_opt(0).unwrap()),
    );
    h.launch();
    assert_eq!(engine.tick().await.unwrap().sent, 1);
    let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
    assert_eq!(files.len(), 1);
    let eml = std::fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
    assert!(eml.contains("Subject: Hello Ana"));
    assert!(eml.contains("NOT sent"));
    assert!(h.fake.sent.lock().unwrap().is_empty());
    let conn = engine.db.lock();
    assert_eq!(accounts::sends_last_24h(&conn, h.account_id, h.clock.now()).unwrap(), 0, "dry run doesn't use the real limit");
    assert_eq!(campaigns::stats(&conn, h.campaign_id).unwrap().dry_run_sent, 1);
}
