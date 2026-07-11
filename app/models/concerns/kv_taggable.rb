# Normalizes and validates a single-level key→value tag map stored in a
# jsonb `tags` column. Keys are strip+downcased; values are stripped strings
# (blank collapses to null) or null. Malformed input is passed through
# untouched so validation rejects it instead of masking it.
module KvTaggable
  extend ActiveSupport::Concern

  MAX_ENTRIES = 50
  MAX_KEY_LENGTH = 100
  MAX_VALUE_LENGTH = 500

  included do
    validate :tags_must_be_a_valid_map
  end

  def tags=(value)
    super(normalize_tags(value))
  end

  private

  def normalize_tags(value)
    return value unless value.is_a?(Hash)
    value.to_h do |key, val|
      key = key.to_s if key.is_a?(Symbol)
      [
        key.is_a?(String) ? key.strip.downcase : key,
        val.is_a?(String) ? val.strip.presence : val
      ]
    end
  end

  def tags_must_be_a_valid_map
    return errors.add(:tags, "must be a map of key/value pairs") unless tags.is_a?(Hash)

    errors.add(:tags, "cannot have more than #{MAX_ENTRIES} entries") if tags.size > MAX_ENTRIES
    unless tags.keys.all? { |k| k.is_a?(String) && k.present? && k.length <= MAX_KEY_LENGTH }
      errors.add(:tags, "keys must be non-empty strings of at most #{MAX_KEY_LENGTH} characters")
    end
    unless tags.values.all? { |v| v.nil? || (v.is_a?(String) && v.length <= MAX_VALUE_LENGTH) }
      errors.add(:tags, "values must be strings of at most #{MAX_VALUE_LENGTH} characters, or null")
    end
  end
end
