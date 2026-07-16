require "socket"

# WebMock intercepts HTTP but not DNS; CallbackUrlPolicy resolves hostnames via
# Addrinfo.getaddrinfo (the same libc resolver Net::HTTP connects with). This
# stubs it deterministically (minitest/mock is no longer bundled with minitest
# 6, hence the hand-rolled singleton swap). Parallel test workers are forked
# processes, so the global swap is safe.
#
# Shapes:
#   stub_dns(["10.0.0.5"]) { ... }       every lookup resolves to these addresses
#   stub_dns { ... }                     any lookup raises (asserts no DNS query)
#   stub_dns!(["1.2.3.4"]) / unstub_dns! setup/teardown variant of the above
module DnsTestHelpers
  # Minimal stand-in for an Addrinfo: the policy only calls #ip_address.
  FakeAddrinfo = Struct.new(:ip_address)

  # Block form: swap for the duration of the block, always restore.
  def stub_dns(addresses = nil)
    original = Addrinfo.method(:getaddrinfo)
    install_getaddrinfo(addresses)
    yield
  ensure
    restore_getaddrinfo(original)
  end

  # setup/teardown form: install a default resolution for a whole test case.
  # Kept separate from the block form's saved handle so a block-form stub can
  # nest inside a stub_dns! default without clobbering the teardown restore.
  def stub_dns!(addresses = nil)
    @dns_original_getaddrinfo = Addrinfo.method(:getaddrinfo)
    install_getaddrinfo(addresses)
  end

  def unstub_dns!
    return unless @dns_original_getaddrinfo

    restore_getaddrinfo(@dns_original_getaddrinfo)
    @dns_original_getaddrinfo = nil
  end

  private

  def install_getaddrinfo(addresses)
    Addrinfo.define_singleton_method(:getaddrinfo) do |_host, *_rest|
      resolved = addresses or raise "DNS must not be queried"
      resolved.map { |address| FakeAddrinfo.new(address) }
    end
  end

  def restore_getaddrinfo(original)
    Addrinfo.singleton_class.remove_method(:getaddrinfo)
    Addrinfo.define_singleton_method(:getaddrinfo, original)
  end
end
