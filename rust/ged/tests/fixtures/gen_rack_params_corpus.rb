# Regenerates rack_params_corpus.json from Rails' own parser, the oracle for
# `ged::http::params`. The Rails stack is the local tag rails-final, not this
# tree. Check that tag out in another worktree and run it there:
#
#   docker compose exec -T web bin/rails runner - \
#     < rust/ged/tests/fixtures/gen_rack_params_corpus.rb \
#     > rust/ged/tests/fixtures/rack_params_corpus.json
#
# (from the repo root; use `bin/worktree-stack exec -T web …` in a worktree).
require "json"

deep = ->(n) { "a" + "[x]" * n + "=1" }
many = ->(n) { (1..n).map { |i| "k#{i}=v" }.join("&") }

corpus = [
  "", "a=1", "a=", "a", "a=1&a=2", "a=1&a=2&a=3", "a=&a=2", "a=1&a=",
  "a[]=1", "a[]=1&a[]=2", "a[]", "a[]=", "a[]&a[]=1", "a[][]=1", "a[][]",
  "a[b]=1", "a[b]=1&a[b]=2", "a[b]=1&a[c]=2", "a[b][c]=1", "a[b][c][d]=1",
  "a[b]", "a[b]=", "a[b][]=1&a[b][]=2", "a[b][]=1&a[c][]=2",
  "a[][b]=1", "a[][b]=1&a[][c]=2", "a[][b]=1&a[][b]=2", "a[][b]=1&a[][b]=2&a[][c]=3",
  "a[][b][c]=1&a[][b][d]=2", "a[][b][]=1&a[][b][]=2",
  "a=1&a[b]=2", "a[b]=1&a=2", "a[]=1&a[b]=2", "a[b]=1&a[]=2", "a[b]=1&a[b][c]=2",
  "a[b][c]=1&a[b]=2", "a=1&a[]=2", "a[]=1&a=2", "a&a[]=1", "a&a[b]=1",
  "tags[type]=facture", "tags[type]=facture&tags[urgent]", "tags[type]=facture&tags[type]=x",
  "tags[a][b]=1", "tags[]=x", "tags=x", "tags[type][]=x",
  "limit[]=1", "limit[]=1&limit[]=2", "limit[a]=1", "limit=5&limit[]=1", "limit[]=1&limit=5",
  "limit=", "limit", "limit=abc&offset=3", "offset[]=",
  "a%20b=c%20d", "a+b=c+d", "a%2Bb=c%2Bd", "a%5Bb%5D=1", "a%5B%5D=1", "%5B=1", "a%5B=1",
  "a[=1", "a]=1", "a[b=1", "a[b]c=1", "a[b]c[d]=1", "a[b][=1", "a[[b]]=1", "a[b[c]]=1",
  "[a]=1", "[]=1", "[a][b]=1", "]=1", "[=1", "=1", "=", "==", "a==b", "a=b=c",
  "&", "&&", "a=1&&b=2", "&a=1&", "a=1& b=2", "a=1&  b=2", "a=1 &b=2", " a=1", "a= 1",
  "a=1;b=2", "a=1,b=2", "a=1#b", "a=%", "a=%2", "a=%zz", "%zz=1", "a=%%41", "a=%4", "a=%41%",
  "a=%ff", "%ff=1", "a=%c3%a9", "%c3%a9=1", "a=%c3", "a=%e2%82%ac", "a=%f0%9f%98%80",
  "a[%ff]=1", "a[b]=%ff", "=%ff", "[]=%ff", "a[]=%ff", "a[]=%ff&b=1",
  "é=1", "é[à]=2", "a=é", "é[]=1", "a%00b=1", "a=%00",
  "a[]=1&b=2&a[]=3", "a=1&b=2&a=3",
  "x[y][z][w]=1&x[y][z][v]=2&x[y][q]=3",
  "a[][b]=1&a[][c]=2&a[][b]=3", "a[][]=1&a[][]=2",
  "a[b][][c]=1&a[b][][d]=2", "a[b][][c]=1&a[b][][c]=2",
  "a[][b]=1&a[]=2", "a[]=1&a[][b]=2",
  "a[b]c[d]=1", "a[]b=1", "a[]b[c]=1", "a[][b]c=1", "a[][b][]=1", "a[][b]=",
  "a[][]b=1", "a[][b][c]d=1",
  deep.(98), deep.(99), deep.(100), deep.(101),
  "a" + "[]" * 50 + "=1", "a" + "[]" * 99 + "=1", "a" + "[]" * 100 + "=1", "a" + "[]" * 101 + "=1",
  many.(10), many.(5000),
  "a=" + "x" * 1000,
].uniq

out = corpus.map do |qs|
  result =
    begin
      { "ok" => ActionDispatch::ParamBuilder.from_query_string(qs).as_json }
    rescue ActionDispatch::ParamError
      { "error" => true }
    end
  { "query" => qs.b.force_encoding("UTF-8").scrub("?"), "raw" => qs, "result" => result }
end

# `raw` keeps the query exactly as sent; the test reads it.
puts JSON.generate(out.map { |e| e.slice("raw", "result") }, max_nesting: false)
