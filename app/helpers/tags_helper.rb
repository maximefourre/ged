module TagsHelper
  # {"type" => "facture", "urgent" => nil} → "type=facture, urgent"
  def format_tags(tags)
    tags.map { |key, value| value.nil? ? key : "#{key}=#{value}" }.join(", ")
  end

  # Textarea form of the map: one key=value pair per line.
  def tags_text(tags)
    tags.map { |key, value| value.nil? ? key : "#{key}=#{value}" }.join("\n")
  end
end
