const LANG_EXAMPLES = [
  {
    id: 'countdown',
    title: 'Countdown',
    src: `-- Words match on the shape of the record flowing through them.
-- Clauses are tried top to bottom; the first match runs.

main = +{n: 3} countdown

countdown {n: 0} = say "liftoff"
countdown        = say "{n}…" n-1 countdown
`,
  },
  {
    id: 'invoice',
    title: 'Invoice',
    src: `-- Invoice collection. The card starts short, the customer tops up
-- while we wait, and the second attempt goes through.

main = +{id: 7, total: 120, card: {kind: Visa, balance: 80}} invoice

invoice  = finalize collect report
finalize = +{due: now + 30, tries: 0}
collect  = charge settle

charge {card} = +{amount: total} gateway
charge        = +{declined: NoCard}

gateway {amount} when card.balance ≥ amount =
  @card( balance=balance - ^amount ) -{amount} +{ok: Approved}
gateway {-amount} = +{declined: Insufficient}

settle {-ok}                             = paid
settle {-declined, tries} when tries < 3 = fail wait collect
settle {-declined}                       = void

fail = tries+1
wait = say "invoice {id}: declined, retry {tries}"
       @card( balance+50 )               -- the customer tops up
paid = -{due, tries} +{paidAt: now}
void = -{due, tries} +{voidedAt: now}

report {paidAt}   = say "invoice {id} paid at step {paidAt}"
report {voidedAt} = say "invoice {id} voided"
`,
  },
  {
    id: 'quote',
    title: 'Quote — tax ‖ shipping',
    src: `-- ‖ runs both sides on the same record and merges what they add.
-- If both sides change the same field, that's an error.

main = +{total: 100, country: ES, weight: 3} quote
       say "{total} + {tax} tax + {shipping} shipping = {grand}"

quote = (tax ‖ shipping) +{grand: total + tax + shipping}

tax {country: ES} = +{tax: total * 0.21}
tax               = +{tax: 0}

shipping when weight > 2 = +{shipping: 12}
shipping                 = +{shipping: 5}
`,
  },
  {
    id: 'order',
    title: 'Order lines — lists',
    src: `-- @field( … ) runs words inside a nested record,
-- or on every item when the field holds a list.

main = +{lines: [{sku: Tea, qty: 2, price: 4.5}, {sku: Pot, qty: 1, price: 30}]}
       price-order show

price-order = @lines( line-total ) +{subtotal: sum(lines.total)} discount

line-total = +{total: qty * price}

discount when subtotal > 35 = +{discount: subtotal * 0.1}
discount                    = +{discount: 0}
`,
  },
  {
    id: 'shape',
    title: 'Shape error',
    src: `-- paid narrows the record, so fail has no tries left to bump.

main = +{id: 7} finalize paid fail

finalize = +{due: now + 30, tries: 0}
paid     = -{due, tries} +{paidAt: now}
fail     = tries+1
`,
  },
];
if (typeof module !== 'undefined') module.exports = LANG_EXAMPLES;
LANG_EXAMPLES.unshift({
  id: 'todos',
  title: 'Todos — SQLite',
  src: `-- A table lives in SQLite. Each run starts from a fresh in-memory
-- database holding the seed rows. The Data tab shows every query.

table todos :: {title: Text, done: Bool}

seed todos = [{title: "Write the parser", done: true},
              {title: "Add SQLite", done: false},
              {title: "Ship the debugger", done: false}]

main = add (+{id: 2} finish) (+{id: 9} finish) list-open tidy

add = +{todo: {title: "Write migrations", done: false}} todos.insert
      say "added #{todo.id}: {todo.title}" -{todo}

finish = +{changes: {done: true}} todos.update finished -{id}

finished {-missing} = say "no todo #{id}"
finished {todo}     = say "finished #{id}: {todo.title}" -{todo}

list-open = todos.where {done: false} when len(title) > 0
            say "{len(todos)} open:" @todos( say "  #{id} {title}" ) -{todos}

tidy = todos.where {done: true} @todos( todos.delete )
       say "removed {len(todos)} finished todos" -{todos}
`,
});
LANG_EXAMPLES.splice(1, 0, {"id":"ledger","title":"Ledger — personal accounting","src":"-- Personal accounting, double-entry. Amounts are cents; debits are positive.\n-- Each route word stands for one HTTP endpoint. The router (not written) puts\n-- path params and query fields at the top level and the JSON body in `body`.\n-- Every route leaves exactly {status, json}.\n\ntable accounts     :: {name: Text, kind: Const, archived: Bool}\ntable transactions :: {date: Text, payee: Text, memo: Text?}\ntable postings     :: {tx-id: Int, account-id: Int, amount: Num, date: Text}\n\nseed accounts = [{name: \"Checking\", kind: Asset, archived: false},\n                 {name: \"Savings\", kind: Asset, archived: false},\n                 {name: \"Credit card\", kind: Liability, archived: false},\n                 {name: \"Salary\", kind: Income, archived: false},\n                 {name: \"Groceries\", kind: Expense, archived: false},\n                 {name: \"Rent\", kind: Expense, archived: false},\n                 {name: \"Opening balances\", kind: Equity, archived: false}]\n\nseed transactions = [{date: \"2026-09-01\", payee: \"Opening balance\"},\n                     {date: \"2026-09-25\", payee: \"Acme Payroll\"},\n                     {date: \"2026-09-27\", payee: \"Landlord\"}]\n\nseed postings = [{tx-id: 1, account-id: 1, amount: 250000, date: \"2026-09-01\"},\n                 {tx-id: 1, account-id: 7, amount: -250000, date: \"2026-09-01\"},\n                 {tx-id: 2, account-id: 1, amount: 310000, date: \"2026-09-25\"},\n                 {tx-id: 2, account-id: 4, amount: -310000, date: \"2026-09-25\"},\n                 {tx-id: 3, account-id: 6, amount: 120000, date: \"2026-09-27\"},\n                 {tx-id: 3, account-id: 1, amount: -120000, date: \"2026-09-27\"}]\n\n-- a session of requests, as the router would hand them over\nmain = (+{id: 1} get-account show-reply)\n       (+{body: {name: \"Restaurants\", kind: Expense}} post-account show-reply)\n       (+{body: {name: \"Pets\", kind: Animal}} post-account show-reply)\n       (+{body: {date: \"2026-10-02\", payee: \"Mercadona\",\n                 postings: [{account-id: 5, amount: 4250}, {account-id: 3, amount: -4250}]}}\n        post-transaction show-reply)\n       (+{body: {date: \"2026-10-03\", payee: \"Typo\",\n                 postings: [{account-id: 5, amount: 1000}, {account-id: 1, amount: -999}]}}\n        post-transaction show-reply)\n       (+{body: {date: \"2026-10-04\", payee: \"Ghost\",\n                 postings: [{account-id: 42, amount: 500}, {account-id: 1, amount: -500}]}}\n        post-transaction show-reply)\n       (+{account: 1} get-transactions show-reply)\n       (+{id: 6} delete-account show-reply)\n       (+{id: 2, body: {archived: true}} patch-account show-reply)\n       (+{id: 3} delete-transaction show-reply)\n       (get-balance-sheet show-reply)\n       (+{from: \"2026-09-01\", to: \"2026-11-01\"} get-income show-reply)\n\nshow-reply = say \"{status}  {json}\" .{}\nreply      = .{status json}\n\n-- GET /accounts\nget-accounts = accounts.where {archived: false} @accounts( with-balance )\n               +{status: 200, json: accounts} reply\n\nwith-balance = postings.where when account-id == ^id\n               +{balance: sum(postings.amount)} -{postings}\n\n-- GET /accounts/:id\nget-account = accounts.get account-found reply\n\naccount-found {-missing} = +{status: 404, json: \"no such account\"}\naccount-found {account}  = @account( with-balance recent ) +{status: 200, json: account}\n\nrecent = postings.where when account-id == ^id +{recent: postings} -{postings}\n\n-- POST /accounts   body: {name, kind}\npost-account {body: {name, kind}} = @body( kind-ok ) create-account reply\npost-account                      = +{status: 422, json: \"name and kind are required\"} reply\n\nkind-ok {kind: Asset}     = nop\nkind-ok {kind: Liability} = nop\nkind-ok {kind: Income}    = nop\nkind-ok {kind: Expense}   = nop\nkind-ok {kind: Equity}    = nop\nkind-ok                   = +{problem: \"kind must be Asset, Liability, Income, Expense or Equity\"}\n\ncreate-account {body: {problem}} = +{status: 422, json: body.problem}\ncreate-account = +{account: {name: body.name, kind: body.kind, archived: false}}\n                 accounts.insert +{status: 201, json: account}\n\n-- PATCH /accounts/:id   body: any of {name, archived}\npatch-account = +{changes: body} -{body} accounts.update account-updated reply\n\naccount-updated {-missing} = +{status: 404, json: \"no such account\"}\naccount-updated {account}  = +{status: 200, json: account}\n\n-- DELETE /accounts/:id   refused while postings use it\ndelete-account = postings.where when account-id == ^id can-delete reply\n\ncan-delete when len(postings) > 0 = +{status: 409, json: \"account has postings — archive it instead\"}\ncan-delete                        = accounts.delete account-deleted\n\naccount-deleted {-missing} = +{status: 404, json: \"no such account\"}\naccount-deleted {-deleted} = +{status: 204, json: nil}\n\n-- GET /transactions?account=\nget-transactions {account} = postings.where when account-id == ^account\n                             @postings( with-payee ) +{status: 200, json: postings} reply\nget-transactions           = transactions.all +{status: 200, json: transactions} reply\n\nwith-payee = -{id} +{id: tx-id} transactions.get attach-payee\n\nattach-payee {transaction} = +{payee: transaction.payee} -{transaction, id}\nattach-payee {-missing}    = +{payee: \"?\"} -{id}\n\n-- GET /transactions/:id\nget-transaction = transactions.get tx-found reply\n\ntx-found {-missing}    = +{status: 404, json: \"no such transaction\"}\ntx-found {transaction} = @transaction( lines ) +{status: 200, json: transaction}\n\nlines = postings.where when tx-id == ^id +{lines: postings} -{postings}\n\n-- POST /transactions   body: {date, payee, postings: [{account-id, amount}]}\npost-transaction {body: {date, payee, postings}} = check-lines reply\npost-transaction = +{status: 422, json: \"date, payee and postings are required\"} reply\n\ncheck-lines when len(body.postings) < 2 =\n  +{status: 422, json: \"a transaction needs at least two postings\"}\ncheck-lines when sum(body.postings.amount) ≠ 0 =\n  +{status: 422, json: \"postings must balance; these sum to {sum(body.postings.amount)}\"}\ncheck-lines = @body( @postings( check-account ) ) count-problems\n\ncheck-account = +{id: account-id} accounts.get usable -{id}\n\nusable {-missing}                  = +{bad: 1}\nusable {account: {archived: true}} = -{account} +{bad: 1}\nusable {-account}                  = +{bad: 0}\n\ncount-problems when sum(body.postings.bad) > 0 =\n  +{status: 422, json: \"postings use missing or archived accounts\"}\ncount-problems = +{transaction: {date: body.date, payee: body.payee}} transactions.insert\n                 @body( @postings( store-posting ) )\n                 +{status: 201, json: transaction}\n\nstore-posting = +{posting: {tx-id: ^^transaction.id, account-id: account-id,\n                            amount: amount, date: ^date}}\n                postings.insert -{posting, bad}\n\n-- DELETE /transactions/:id\ndelete-transaction = postings.where when tx-id == ^id @postings( postings.delete .{id} )\n                     -{postings} transactions.delete tx-deleted reply\n\ntx-deleted {-missing} = +{status: 404, json: \"no such transaction\"}\ntx-deleted {-deleted} = +{status: 204, json: nil}\n\n-- GET /reports/balance-sheet\nget-balance-sheet =\n  +{assets: {kind: Asset}, liabilities: {kind: Liability}, equity: {kind: Equity}}\n  (@assets( kind-total ) ‖ @liabilities( kind-total ) ‖ @equity( kind-total ))\n  +{status: 200, json: {assets: assets.total, liabilities: liabilities.total,\n                        equity: equity.total, net-worth: assets.total + liabilities.total}}\n  reply\n\nkind-total = accounts.where when kind == ^kind @accounts( with-balance )\n             +{total: sum(accounts.balance)}\n\n-- GET /reports/income?from=&to=   dates as text, to is exclusive\nget-income {from, to} = +{income: {kind: Income}, expenses: {kind: Expense}}\n                        (@income( kind-flow ) ‖ @expenses( kind-flow ))\n                        +{status: 200, json: {income: -income.total, expenses: expenses.total,\n                                              net: -(income.total + expenses.total),\n                                              by-account: income.accounts ++ expenses.accounts}}\n                        reply\nget-income = +{status: 400, json: \"from and to are required\"} reply\n\nkind-flow = accounts.where when kind == ^kind @accounts( flow .{name flow} )\n            +{total: sum(accounts.flow)}\n\n-- inside a where, ^ is the record being processed; ^^^ is two records further out\nflow = postings.where when account-id == ^id and date ≥ ^^^from and date < ^^^to\n       +{flow: sum(postings.amount)} -{postings}\n"});

LANG_EXAMPLES.splice(2, 0, {"id": "customers", "title": "Customers — record algebra", "src": "-- The record algebra on a small customer service. Every change to a customer\n-- is stored as a ∆, so any past version can be rebuilt by replaying them.\n-- Router: path params at the top level, the JSON body in `body`,\n-- the signed-in customer in `auth`, the request time in `time`.\n\ntable customers :: {name: Text, email: Text, plan: Const, state: Const}\ntable revisions :: {customer-id: Int, at: Text, change: Json}\ntable events    :: {key: Text, customer-id: Int, metric: Const, qty: Num}\n\nmain = (+{time: \"2026-10-01\", body: {name: \"Ana\", email: \"ana@x.com\", plan: Free}} post-customer show-reply)\n       (+{time: \"2026-10-03\", id: 1, body: {email: \"ana@ana.dev\"}} patch-customer show-reply)\n       (+{time: \"2026-10-05\", id: 1, body: {plan: Pro, state: Paused, shoe-size: 41}} patch-customer show-reply)\n       (+{id: 1, at: \"2026-10-04\"} get-as-of show-reply)\n       (+{auth: {customer-id: 1}, body: {key: \"k1\", metric: Api, qty: 120}} post-event show-reply)\n       (+{auth: {customer-id: 1}, body: {key: \"k1\", metric: Api, qty: 120}} post-event show-reply)\n       (+{auth: {customer-id: 1}, body: {key: \"k1\", metric: Api, qty: 999}} post-event show-reply)\n       (+{auth: {customer-id: 1}, body: {key: \"k2\", customer-id: 2, metric: Api, qty: 5}} post-event show-reply)\n\nshow-reply = say \"{status}  {json}\" .{}\nreply      = .{status json}\n\n-- POST /customers   ⊃ lifts the body's fields up, ⊂ packs the row back up\npost-customer {body: {name, email, plan}} =\n  ⊃body +{state: Active} ⊂customer{name email plan state}\n  customers.insert +{change: {} ∆ customer} log\n  +{status: 201, json: customer} reply\npost-customer = +{status: 422, json: \"name, email and plan are required\"} reply\n\n-- PATCH /customers/:id   body: any of {name, email, plan, state}; anything else is ignored\npatch-customer = customers.get patched reply\n\npatched {-missing} = +{status: 404, json: \"no such customer\"}\npatched {customer} =\n  +{change: customer ∆ (customer ⫽ body.{name email plan state})} +{changes: change}\n  -{customer, body} customers.update updated\n\nupdated {-missing} = +{status: 404, json: \"no such customer\"}\nupdated {customer} = log +{status: 200, json: customer}\n\n-- every change is kept as the ∆ that made it\nlog = +{revision: {customer-id: customer.id, at: time, change: change}}\n      revisions.insert -{revision, change}\n\n-- GET /customers/:id/as-of?at=   replay the stored ∆s onto an empty record\nget-as-of = revisions.where when customer-id == ^id and at ≤ ^at\n            +{rebuilt: {}} replay +{status: 200, json: rebuilt} reply\n\nreplay {revisions: []} = -{revisions}\nreplay = rebuilt=(rebuilt ⍎ head(revisions).change) revisions=tail(revisions) replay\n\n-- POST /events   idempotent on key; customer-id comes from auth when the body leaves it out\npost-event {body: {key, metric, qty}} = @body( ⊔{customer-id: ^auth.customer-id} ) owned\npost-event = +{status: 422, json: \"key, metric and qty are required\"} reply\n\nowned {body: {conflict}} = +{status: 403, json: \"that customer isn't you\"} reply\nowned = body→event events.where when key == ^event.key ingest reply\n\ningest when events == []         = events.insert +{status: 201, json: event}\ningest when event ⊑ head(events) = +{status: 200, json: head(events)}\ningest = +{status: 409, json: event ⊓ head(events)}\n"});
