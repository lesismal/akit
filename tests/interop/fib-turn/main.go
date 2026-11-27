// Command fib-turn is fib's TURN client (fit's, in its tests and its p2p
// package), driven against a TURN server for akit's interop tests (I111):
// it allocates over UDP, TCP or TLS, relays each message to an echoing peer
// by Send indication or by channel, and reads the echo back.
//
//	fib-turn -server 127.0.0.1:3478 -transport udp -user alice -pass pw \
//	    -peer 127.0.0.1:9000 -bind -n 3
//
// It prints "relayed <address>" once allocated, "echo <i>" for each echo,
// and "ok" at the end; a refusal prints "error <code>" and exits 2.
package main

import (
	"crypto/tls"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/netip"
	"os"
	"sync"
	"time"

	fibturn "github.com/lesismal/fib/webrtc/turn"
)

func main() {
	server := flag.String("server", "", "the TURN server's address")
	transport := flag.String("transport", "udp", "udp, tcp or tls")
	user := flag.String("user", "", "the username")
	pass := flag.String("pass", "", "the password")
	secret := flag.String("secret", "", "make time-limited credentials from this secret instead")
	peer := flag.String("peer", "", "the echoing peer's address")
	bind := flag.Bool("bind", false, "relay by channel")
	n := flag.Int("n", 3, "messages")
	flag.Parse()
	if err := run(*server, *transport, *user, *pass, *secret, *peer, *bind, *n); err != nil {
		var te *fibturn.Error
		if errors.As(err, &te) {
			fmt.Printf("error %d\n", te.Code)
			os.Exit(2)
		}
		fmt.Println("failed:", err)
		os.Exit(1)
	}
	fmt.Println("ok")
}

type event struct {
	relayed netip.AddrPort
	data    []byte
	err     error
}

func run(server, transport, user, pass, secret, peer string, bind bool, n int) error {
	srv, err := netip.ParseAddrPort(server)
	if err != nil {
		return err
	}
	if secret != "" {
		user, pass = fibturn.EphemeralCredentials([]byte(secret), user, time.Now().Add(time.Hour))
	}
	events := make(chan event, 64)
	var (
		mu     sync.Mutex
		send   func([]byte)
		reader func() ([]byte, error)
	)
	switch transport {
	case "udp":
		c, err := net.ListenUDP("udp", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
		if err != nil {
			return err
		}
		defer c.Close()
		send = func(b []byte) { _, _ = c.WriteToUDPAddrPort(b, srv) }
		buf := make([]byte, 65536)
		reader = func() ([]byte, error) {
			k, _, err := c.ReadFromUDPAddrPort(buf)
			return buf[:k], err
		}
	case "tcp", "tls":
		var c net.Conn
		if transport == "tls" {
			c, err = tls.Dial("tcp", server, &tls.Config{InsecureSkipVerify: true, ServerName: "localhost"})
		} else {
			c, err = net.Dial("tcp", server)
		}
		if err != nil {
			return err
		}
		defer c.Close()
		send = func(b []byte) { _, _ = c.Write(b) }
		var pending []byte
		buf := make([]byte, 65536)
		reader = func() ([]byte, error) {
			for {
				if k, err := fibturn.FrameLen(pending); err != nil {
					return nil, err
				} else if k > 0 && len(pending) >= k {
					m := append([]byte(nil), pending[:k]...)
					pending = pending[k:]
					return m, nil
				}
				r, err := c.Read(buf)
				if err != nil {
					return nil, err
				}
				pending = append(pending, buf[:r]...)
			}
		}
	default:
		return fmt.Errorf("transport %q", transport)
	}
	client, err := fibturn.NewClient(fibturn.ClientConfig{
		Server: srv, Username: user, Password: pass, BindChannels: bind,
		Stream: transport != "udp",
		Send:   func(_ netip.AddrPort, b []byte) { send(append([]byte(nil), b...)) },
		OnAllocated: func(relayed, _ netip.AddrPort) {
			events <- event{relayed: relayed}
		},
		OnFailed: func(err error) { events <- event{err: err} },
	})
	if err != nil {
		return err
	}
	do := func(f func(*fibturn.Client)) {
		mu.Lock()
		defer mu.Unlock()
		f(client)
	}
	go func() {
		for {
			b, err := reader()
			if err != nil {
				events <- event{err: err}
				return
			}
			var data []byte
			do(func(c *fibturn.Client) {
				if _, d, ok := c.HandlePacket(srv, b, time.Now()); ok && d != nil {
					data = append([]byte(nil), d...)
				}
			})
			if data != nil {
				events <- event{data: data}
			}
		}
	}()
	go func() {
		for range time.Tick(50 * time.Millisecond) {
			do(func(c *fibturn.Client) { c.HandleTimeout(time.Now()) })
		}
	}()
	do(func(c *fibturn.Client) { err = c.Allocate(time.Now()) })
	if err != nil {
		return err
	}
	var relayed netip.AddrPort
	select {
	case e := <-events:
		if e.err != nil {
			return e.err
		}
		relayed = e.relayed
	case <-time.After(10 * time.Second):
		return errors.New("no allocation")
	}
	fmt.Println("relayed", relayed)
	if peer == "" {
		return nil
	}
	pa, err := netip.ParseAddrPort(peer)
	if err != nil {
		return err
	}
	for i := 0; i < n; i++ {
		msg := fmt.Sprintf("message %d", i)
		deadline := time.After(10 * time.Second)
	resend:
		for {
			var serr error
			do(func(c *fibturn.Client) { serr = c.SendTo(pa, []byte(msg)) })
			if serr != nil {
				return serr
			}
			select {
			case e := <-events:
				if e.err != nil {
					return e.err
				}
				if string(e.data) == "echo:"+msg {
					fmt.Println("echo", i)
					break resend
				}
			case <-time.After(200 * time.Millisecond):
			case <-deadline:
				return fmt.Errorf("no echo of %q", msg)
			}
		}
	}
	do(func(c *fibturn.Client) { c.Close(time.Now()) })
	time.Sleep(100 * time.Millisecond)
	return nil
}
