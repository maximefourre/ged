Rails.application.routes.draw do
  devise_for :admins
  get "up" => "rails/health#show", as: :rails_health_check

  namespace :admin do
    root to: "dashboard#index"
  end
  root to: redirect("/admin")

  namespace :api do
    namespace :v1 do
      get "ping", to: "pings#show"
      resources :documents, only: [ :index, :create, :show, :destroy ] do
        member { get :download }
      end
    end
  end
end
