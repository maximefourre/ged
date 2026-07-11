require "test_helper"

class TagsMapValidationTest < ActiveSupport::TestCase
  setup do
    @finance = Department.create!(name: "Finance")
  end

  def build_document(tags)
    Document.new(department: @finance, filename: "r.pdf", uploaded_by: "seed", tags: tags)
  end

  test "accepts a flat map of string keys with string or null values" do
    assert build_document({ "type" => "facture", "urgent" => nil }).valid?
    assert build_document({}).valid?
  end

  test "normalizes keys (strip+downcase) and values (strip, blank becomes null)" do
    doc = build_document({ "  Type " => "  Facture ", "urgent" => "   " })
    assert_equal({ "type" => "Facture", "urgent" => nil }, doc.tags)
  end

  test "symbol keys are stringified" do
    assert_equal({ "type" => "facture" }, build_document({ type: "facture" }).tags)
  end

  test "rejects values that are not a map" do
    [ nil, "flat", 42, [ "a", "b" ] ].each do |bad|
      doc = build_document(bad)
      assert_not doc.valid?, "expected #{bad.inspect} to be invalid"
      assert doc.errors[:tags].any?
    end
  end

  test "rejects nested or non-string values" do
    [ { "k" => { "nested" => "x" } }, { "k" => [ "x" ] }, { "k" => 42 }, { "k" => true } ].each do |bad|
      assert_not build_document(bad).valid?, "expected #{bad.inspect} to be invalid"
    end
  end

  test "rejects blank and overlong keys, and overlong values" do
    assert_not build_document({ "  " => "x" }).valid?
    assert_not build_document({ "k" * 101 => "x" }).valid?
    assert_not build_document({ "k" => "v" * 501 }).valid?
  end

  test "rejects more than 50 entries" do
    doc = build_document(51.times.to_h { |i| [ "key-#{i}", nil ] })
    assert_not doc.valid?
  end

  test "upload requests validate tags the same way" do
    request = UploadRequest.new(department: @finance, filename: "r.pdf",
                                callback_url: "https://caller.test/cb",
                                uploaded_by: "erp-app", tags: "flat")
    assert_not request.valid?
    request.tags = { "Type" => " facture " }
    assert request.valid?
    assert_equal({ "type" => "facture" }, request.tags)
  end
end
