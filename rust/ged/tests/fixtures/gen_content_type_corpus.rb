# Content-type corpus: what Active Storage's identification (Marcel, exactly as
# ActiveStorage::Blob#extract_content_type calls it) answers for each input.
# Rust's `detect_content_type` must reproduce every line.
#
#   bundle exec ruby rust/ged/tests/fixtures/gen_content_type_corpus.rb
require "marcel"
require "stringio"
require "json"

pdf  = "%PDF-1.4\n1 0 obj\n<<>>\nendobj\n"
png  = "\x89PNG\r\n\x1A\n".b + "\x00\x00\x00\rIHDR".b + "\x00" * 20
svg  = '<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>'
svg2 = '<svg xmlns="http://www.w3.org/2000/svg"></svg>'
html = "<!DOCTYPE html><html><body>hi</body></html>"
zip  = "PK\x03\x04".b + "\x14\x00\x00\x00\x08\x00".b + "\x00" * 30
bin  = (0..255).map(&:chr).join.b
txt  = "hello world\n"
jpg  = "\xFF\xD8\xFF\xE0\x00\x10JFIF\x00".b + "\x00" * 20
gif  = "GIF89a".b + "\x00" * 20
ole  = "\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1".b + "\0" * 30
docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
pk   = "PK\x03\x04".b + "\0" * 30
xml  = "<?xml version=\"1.0\"?>"
rng  = Random.new(42)

cases = [
  # [label, data, filename, declared]
  ["pdf", pdf, "a.pdf", nil], ["pdf_named_txt", pdf, "a.txt", nil], ["pdf_declared_png", pdf, "a.png", "image/png"],
  ["pdf_leading_space", " " + pdf, "a", nil], ["pdf_at_512", " " * 512 + pdf, "a", nil], ["pdf_at_1000", "x" * 1000 + pdf, "a", nil],
  ["png", png, "a.png", nil], ["png_declared_pdf", png, "a.pdf", "application/pdf"], ["png_upper_ext", png, "A.PNG", nil],
  ["svg_xml_prolog", svg, "a.svg", nil], ["svg", svg2, "a.svg", nil], ["svg_no_ext", svg2, "a", nil], ["svg_declared_png", svg2, "a.png", "image/png"],
  ["svg_declared_html", "<svg/>", "a.html", "text/html"], ["svg_prolog_selfclosing", xml + "<svg/>", "a", nil],
  ["svg_inside_root", xml + "<root><svg/></root>", "a", nil], ["svg_comment_first", "<!-- x --><svg/>", "a", nil],
  ["svg_late", xml + "\n" + " " * 2000 + "<svg/>", "a", nil], ["svg_two_spaces", "  <svg/>", "a", nil], ["svg_one_space", " <svg/>", "a", nil],
  ["svg_upper", "<SVG/>", "a", nil], ["svg_doctype", "<!DOCTYPE svg><svg/>", "a", nil],
  ["svg_xmlns_then_attr", "<svg xmlns='http://www.w3.org/2000/svg'/>", "a", nil],
  ["html", html, "a.html", nil], ["html_named_txt", html, "a.txt", nil], ["html_no_ext", html, "a", nil],
  ["html_declared_png", html, "a.png", "image/png"], ["html_script", "<html><script>alert(1)</script></html>", "a.html", "text/html"],
  ["html_upper", "<HTML><BODY>", "a", nil], ["html_mixed_case", "<Html>", "a", nil], ["html_lower_doctype", "<!doctype html><p>", "a.txt", nil],
  ["html_leading_ws", "  \n<html><body>", "a", nil], ["html_leading_newline", "\n<html>", "a", nil], ["html_div", "<div>hi</div>", "a.txt", nil],
  ["html_script_only", "<script>alert(1)</script>", "a.txt", nil], ["html_body", "<body>x</body>", "a", nil], ["html_head", "<head><title>x</title>", "a", nil],
  ["html_comment_first", "<!-- c --><html>", "a", nil], ["html_bom", "\xEF\xBB\xBF<html>".b, "a", nil], ["html_with_svg", "<html><svg/></html>", "a", nil],
  ["zip", zip, "a.zip", nil], ["zip_named_docx", zip, "a.docx", nil], ["zip_named_pdf", zip, "a.pdf", nil], ["zip_named_pptx", pk, "a.pptx", nil],
  ["zip_named_odt", pk, "a.odt", nil], ["zip_named_jar", pk, "a.jar", nil], ["zip_named_xlsx_declared", zip, "a.xlsx", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"],
  ["zip_declared_docx", pk, "a", docx], ["zip_named_zip_declared_docx", pk, "a.zip", docx], ["empty_zip", "PK\x05\x06".b + "\0" * 18, "a.zip", nil],
  ["text_named_docx", txt, "a.docx", nil], ["ole", ole, "a.doc", nil], ["ole_named_xls", ole, "a.xls", nil], ["gzip", "\x1F\x8B\x08".b + "\0" * 10, "a.gz", nil],
  ["binary", bin, "a.bin", nil], ["binary_no_ext", bin, "a", nil], ["binary_declared_png", bin, "a.png", "image/png"], ["binary_declared_pdf", bin, "x", "application/pdf"],
  ["binary_declared_octet", bin, "x", "application/octet-stream"],
  ["text", txt, "a.txt", nil], ["text_named_pdf", txt, "a.pdf", nil], ["text_declared_html", txt, "a.txt", "text/html"], ["text_declared_junk", txt, "a.txt", "junk"],
  ["text_declared_upper_params", "hi", "a", "TEXT/PLAIN; charset=utf-8"], ["text_declared_no_slash", "hi", "a", "text"], ["text_declared_octet", "hi", "a", "application/octet-stream"],
  ["text_declared_leading_space", "hi", "a.txt", " text/html"], ["text_declared_comma", "hi", "a", "text/csv, text/plain"],
  ["jpeg_named_txt", jpg, "a.txt", "text/plain"], ["gif_named_pdf", gif, "x.pdf", nil], ["empty_declared_pdf", "", "a.pdf", "application/pdf"], ["empty_no_ext", "", "a", nil],
  ["json", '{"a":1}', "a.json", nil], ["json_declared_octet", '{"a":1}', "a.json", "application/octet-stream"], ["csv", "a,b\n1,2\n", "a.csv", nil],
  ["xml", xml + "<root/>", "a.xml", nil], ["xml_no_ext", xml + "<root/>", "a", nil],
  ["name_dotfile", "hi", ".pdf", nil], ["name_hidden_with_ext", "hi", ".a.pdf", nil], ["name_double_ext", "hi", "a.tar.gz", nil], ["name_trailing_dot", "hi", "a.pdf.", nil],
  ["name_leading_dots", "hi", "..pdf", nil], ["name_path", "hi", "../x/a.pdf", nil], ["name_trailing_space", "hi", "a.pdf ", nil], ["name_forbidden_chars", "hi", "a:b?.pdf", nil],
  ["name_forbidden_in_ext", "hi", "a.p?df", nil], ["name_nil", pdf, nil, nil], ["name_nil_text", txt, nil, nil],
  ["mp4", "\0\0\0\x18ftypmp42".b + "\0" * 20, "a.mp4", nil], ["webp", "RIFF\0\0\0\0WEBPVP8 ".b, "a", nil],
  ["bzip2", "BZh9".b + "\0" * 8, "a", nil], ["java_profile", "JAVA PROFILE 1.0.2\0".b, "a", nil],
  ["unicode_name", "hi", "résumé.pdf", nil], ["mixed_case_declared", "hi", "a", "Image/PNG"],
]
8.times { |i| cases << ["random_#{i}", rng.bytes(rng.rand(1..600)), "r#{i}", nil] }
8.times { |i| cases << ["random_named_#{i}", rng.bytes(rng.rand(1..600)), "r#{i}.#{%w[pdf png txt docx zip html svg csv][i]}", nil] }

sanitize = ->(name) { name.to_s.encode(Encoding::UTF_8, invalid: :replace, undef: :replace, replace: "�").strip.tr("\u{202E}%$|:;/<>?*\"\\", "-") }
out = cases.map do |label, data, name, declared|
  data = data.b
  # ActiveStorage::Blob#extract_content_type: name is the sanitized Filename#to_s.
  expected = Marcel::MimeType.for(StringIO.new(data), name: sanitize.(name), declared_type: declared)
  { label:, data_b64: [data].pack("m0"), filename: name, declared:, expected: }
end
path = File.expand_path("content_type_corpus.json", __dir__)
File.write(path, JSON.pretty_generate({ marcel: Marcel::VERSION, cases: out }) + "\n")
puts "wrote #{out.size} cases (marcel #{Marcel::VERSION})"
