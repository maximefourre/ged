# WebMock intercepts HTTP but not DNS; CallbackUrlPolicy resolves hostnames
# via Resolv.getaddresses. This stubs it deterministically (minitest/mock is
# no longer bundled with minitest 6, hence the hand-rolled singleton swap).
# Parallel test workers are forked processes, so the global swap is safe.
module DnsTestHelpers
  # stub_dns(["10.0.0.5"]) { ... }  — every lookup resolves to these addresses
  # stub_dns { ... }                — any lookup raises (asserts no DNS query)
  def stub_dns(addresses = nil)
    original = Resolv.method(:getaddresses)
    Resolv.define_singleton_method(:getaddresses) do |_host|
      addresses or raise "DNS must not be queried"
    end
    yield
  ensure
    Resolv.singleton_class.remove_method(:getaddresses)
    Resolv.define_singleton_method(:getaddresses, original)
  end
end
